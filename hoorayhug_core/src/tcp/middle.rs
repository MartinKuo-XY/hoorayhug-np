use std::io::Result;
use tokio::net::TcpStream;

use super::socket;
use super::plain;

#[cfg(feature = "hook")]
use super::hook;

#[cfg(feature = "proxy")]
use super::proxy;

#[cfg(feature = "transport")]
use super::transport;

use crate::trick::Ref;
use crate::endpoint::{RemoteAddr, ConnectOpts};

#[cfg(feature = "balance")]
async fn try_connect_with_fallback(
    balancer: &hoorayhug_lb::Balancer,
    raddr: &RemoteAddr,
    extra_raddrs: &[RemoteAddr],
    conn_opts: &ConnectOpts,
    local: &TcpStream,
) -> Result<TcpStream> {
    use hoorayhug_lb::{Token, BalanceCtx};
    use std::collections::HashSet;

    let src_ip = local.peer_addr()?.ip();
    let mut token = balancer.next(BalanceCtx { src_ip: &src_ip });
    let mut tried: HashSet<u8> = HashSet::new();
    let total = balancer.total();

    loop {
        let selected = match token {
            None | Some(Token(0)) => raddr,
            Some(Token(idx)) => {
                let i = idx as usize;
                if i > 0 && i - 1 < extra_raddrs.len() {
                    &extra_raddrs[i - 1]
                } else {
                    raddr
                }
            }
        };

        log::debug!("[tcp]select remote peer, token: {:?}", token);

        match super::socket::connect(selected, conn_opts).await {
            Ok(stream) => {
                if let Some(tok) = token {
                    balancer.on_success(tok);
                }
                log::info!("[tcp]{} => {} as {}", local.peer_addr()?, selected, stream.peer_addr()?);
                return Ok(stream);
            }
            Err(e) => {
                log::warn!("[tcp]connect to {} failed: {}, trying next peer", selected, e);
                if let Some(tok) = token {
                    balancer.on_failure(tok);
                    tried.insert(tok.0);
                }
                // All peers exhausted — give up.
                if total == 0 || tried.len() as u8 >= total {
                    return Err(e);
                }
                token = balancer.next(BalanceCtx { src_ip: &src_ip });
            }
        }
    }
}

#[allow(unused)]
pub async fn connect_and_relay(
    mut local: TcpStream,
    raddr: Ref<RemoteAddr>,
    conn_opts: Ref<ConnectOpts>,
    extra_raddrs: Ref<Vec<RemoteAddr>>,
) -> Result<()> {
    let ConnectOpts {
        #[cfg(feature = "proxy")]
        proxy_opts,

        #[cfg(feature = "transport")]
        transport,

        #[cfg(feature = "balance")]
        balancer,

        tcp_keepalive,
        ..
    } = conn_opts.as_ref();

    // pre-connect hook (called once before any connection attempt).
    #[cfg(feature = "hook")]
    hook::pre_connect_hook(&mut local, raddr.as_ref(), extra_raddrs.as_ref()).await?;

    // connect with optional retry (balance feature).
    let mut remote = {
        #[cfg(feature = "balance")]
        {
            try_connect_with_fallback(
                balancer,
                raddr.as_ref(),
                extra_raddrs.as_ref(),
                conn_opts.as_ref(),
                &local,
            )
            .await?
        }

        #[cfg(not(feature = "balance"))]
        {
            let remote = socket::connect(raddr.as_ref(), conn_opts.as_ref()).await?;
            log::info!("[tcp]{} => {} as {}", local.peer_addr()?, raddr.as_ref(), remote.peer_addr()?);
            remote
        }
    };

    // after connected
    // ..
    #[cfg(feature = "proxy")]
    if proxy_opts.enabled() {
        proxy::handle_proxy(&mut local, &mut remote, *proxy_opts).await?;
    }

    // relay
    let res = {
        #[cfg(feature = "transport")]
        {
            if let Some((ac, cc)) = transport {
                transport::run_relay(local, remote, ac, cc).await
            } else {
                plain::run_relay(local, remote).await
            }
        }
        #[cfg(not(feature = "transport"))]
        {
            plain::run_relay(local, remote).await
        }
    };

    // ignore relay error
    if let Err(e) = res {
        log::debug!("[tcp]forward error: {}, ignored", e);
    }

    Ok(())
}

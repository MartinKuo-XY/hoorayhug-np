//! TCP relay entrance.

mod socket;
mod middle;
mod plain;

#[cfg(feature = "hook")]
mod hook;

#[cfg(feature = "proxy")]
mod proxy;

#[cfg(feature = "transport")]
mod transport;

use std::io::{ErrorKind, Result};

use crate::trick::Ref;
use crate::endpoint::Endpoint;

use middle::connect_and_relay;

/// Launch a tcp relay.
pub async fn run_tcp(endpoint: Endpoint) -> Result<()> {
    let Endpoint {
        laddr,
        raddr,
        bind_opts,
        conn_opts,
        extra_raddrs,
    } = endpoint;

    let raddr = Ref::new(&raddr);
    let conn_opts = Ref::new(&conn_opts);
    let extra_raddrs = Ref::new(&extra_raddrs);

    let lis = socket::bind(&laddr, bind_opts).unwrap_or_else(|e| panic!("[tcp]failed to bind {}: {}", &laddr, e));
    let keepalive = socket::keepalive::build(&conn_opts);

    loop {
        let (local, addr) = match lis.accept().await {
            Ok(x) => x,
            Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                log::warn!("[tcp]failed to accept: {}", e);
                continue;
            }
            Err(e) => {
                log::error!("[tcp]failed to accept: {}", e);
                break;
            }
        };

        // ============================================================
        // 【修改点】: 彻底删除了 let _ = local.set_nodelay(true);
        // 保留 Nagle 算法，使得混淆首包和真实数据能被内核合并为一个包发送
        // ============================================================
        
        if let Some(kpa) = &keepalive {
            use socket::keepalive::SockRef;
            SockRef::from(&local).set_tcp_keepalive(kpa)?;
        }

        // 挂载你设计的协议上下文
        let mode_num = match conn_opts.obfs.as_str() {
            "client" => 1,
            "server" => 2,
            _ => 0,
        };

        tokio::spawn(async move {
            hoorayhug_io::mem_copy::OBFS_MODE.scope(mode_num, async move {
                match connect_and_relay(local, raddr, conn_opts, extra_raddrs).await {
                    Ok(..) => log::debug!("[tcp]{} => {}, finish", addr, raddr.as_ref()),
                    Err(e) => log::error!("[tcp]{} => {}, error: {}", addr, raddr.as_ref(), e),
                }
            }).await;
        });
    }

    Ok(())
}

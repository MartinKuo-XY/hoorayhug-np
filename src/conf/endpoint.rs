use serde::{Serialize, Deserialize};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use hoorayhug_core::endpoint::{Endpoint, RemoteAddr};

#[cfg(feature = "balance")]
use hoorayhug_core::balance::{Balancer, HealthCheckConfig};

#[cfg(feature = "transport")]
use hoorayhug_core::kaminari::mix::{MixAccept, MixConnect};

use super::{Config, NetConf, NetInfo};

#[derive(Debug, Serialize, Deserialize)]
pub struct EndpointConf {
    pub listen: String,
    pub remote: String,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_remotes: Vec<String>,

    // ======== 解析配置文件的 obfs 字段 ========
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obfs: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_interface: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_transport: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_transport: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Config::is_empty")]
    pub network: NetConf,
}

impl EndpointConf {
    fn build_local(&self) -> SocketAddr {
        self.listen.to_socket_addrs().expect("invalid local address").next().unwrap()
    }

    fn build_remote(&self) -> RemoteAddr {
        Self::build_remote_x(&self.remote)
    }

    fn build_remote_x(remote: &str) -> RemoteAddr {
        if let Ok(sockaddr) = remote.parse::<SocketAddr>() {
            RemoteAddr::SocketAddr(sockaddr)
        } else {
            let mut iter = remote.rsplitn(2, ':');
            let port = iter.next().unwrap().parse::<u16>().unwrap();
            let addr = iter.next().unwrap().to_string();
            RemoteAddr::DomainName(addr, port)
        }
    }

    fn build_send_through(&self) -> Option<SocketAddr> {
        let Self { through, .. } = self;
        let through = match through { Some(x) => x, None => return None };
        match through.to_socket_addrs() {
            Ok(mut x) => Some(x.next().unwrap()),
            Err(_) => {
                let mut ipstr = String::from(through);
                ipstr.retain(|c| c != '[' && c != ']');
                ipstr.parse::<IpAddr>().map_or(None, |ip| Some(SocketAddr::new(ip, 0)))
            }
        }
    }

    #[cfg(feature = "balance")]
    fn build_balancer(&self, max_fails: Option<u32>, fail_timeout: Option<u32>, max_latency: Option<u32>) -> Balancer {
        let hc = match (max_fails, fail_timeout) {
            (Some(max_fails), Some(fail_timeout)) => Some(HealthCheckConfig { max_fails, fail_timeout_secs: fail_timeout, max_latency_ms: max_latency }),
            _ => None,
        };
        if let Some(s) = &self.balance {
            // Parse strategy + weights, then reconstruct with health config.
            let (strategy, weights) = s.split_once(':').unwrap_or((s, ""));
            use hoorayhug_core::balance::Strategy;
            let strategy = Strategy::from(strategy.trim());
            let weights: Vec<u8> = weights
                .trim()
                .split(',')
                .filter_map(|w| w.trim().parse().ok())
                .collect();
            Balancer::new(strategy, &weights, hc)
        } else {
            Balancer::default()
        }
    }

    #[cfg(feature = "transport")]
    fn build_transport(&self) -> Option<(MixAccept, MixConnect)> {
        use hoorayhug_core::kaminari::mix::{MixClientConf, MixServerConf};
        use hoorayhug_core::kaminari::opt::{get_ws_conf, get_tls_client_conf, get_tls_server_conf};

        let Self { listen_transport, remote_transport, .. } = self;
        let listen_ws = listen_transport.as_ref().and_then(|s| get_ws_conf(s));
        let listen_tls = listen_transport.as_ref().and_then(|s| get_tls_server_conf(s));
        let remote_ws = remote_transport.as_ref().and_then(|s| get_ws_conf(s));
        let remote_tls = remote_transport.as_ref().and_then(|s| get_tls_client_conf(s));

        if matches!((&listen_ws, &listen_tls, &remote_ws, &remote_tls), (None, None, None, None)) {
            None
        } else {
            let ac = MixAccept::new_shared(MixServerConf { ws: listen_ws, tls: listen_tls });
            let cc = MixConnect::new_shared(MixClientConf { ws: remote_ws, tls: remote_tls });
            Some((ac, cc))
        }
    }
}

#[derive(Debug)]
pub struct EndpointInfo {
    pub no_tcp: bool,
    pub use_udp: bool,
    pub endpoint: Endpoint,
}

impl Config for EndpointConf {
    type Output = EndpointInfo;
    fn is_empty(&self) -> bool { false }
    fn build(self) -> Self::Output {
        let laddr = self.build_local();
        let raddr = self.build_remote();
        let extra_raddrs = self.extra_remotes.iter().map(|r| Self::build_remote_x(r)).collect();
        let NetInfo { mut bind_opts, mut conn_opts, no_tcp, use_udp, max_fails, fail_timeout, max_latency } = self.network.build();

        #[cfg(feature = "balance")] { conn_opts.balancer = self.build_balancer(max_fails, fail_timeout, max_latency); }
        #[cfg(feature = "transport")] { conn_opts.transport = self.build_transport(); }

        conn_opts.bind_address = self.build_send_through();
        conn_opts.bind_interface = self.interface;
        bind_opts.bind_interface = self.listen_interface;
        
        conn_opts.obfs = self.obfs.unwrap_or_default().to_lowercase();

        EndpointInfo { no_tcp, use_udp, endpoint: Endpoint { laddr, raddr, bind_opts, conn_opts, extra_raddrs } }
    }
    fn rst_field(&mut self, _: &Self) -> &mut Self { unreachable!() }
    fn take_field(&mut self, _: &Self) -> &mut Self { unreachable!() }
    fn from_cmd_args(matches: &clap::ArgMatches) -> Self {
        EndpointConf {
            listen: matches.get_one::<String>("local").cloned().unwrap(),
            remote: matches.get_one::<String>("remote").cloned().unwrap(),
            through: matches.get_one::<String>("through").cloned(),
            interface: matches.get_one::<String>("interface").cloned(),
            listen_interface: matches.get_one::<String>("listen_interface").cloned(),
            listen_transport: matches.get_one::<String>("listen_transport").cloned(),
            remote_transport: matches.get_one::<String>("remote_transport").cloned(),
            network: Default::default(), extra_remotes: Vec::new(), balance: None,
            obfs: None,
        }
    }
}

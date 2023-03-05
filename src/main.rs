#![feature(type_alias_impl_trait)]

use std::{collections::HashMap, process::exit};

use clap::{Parser, Subcommand};
use tracing_subscriber::{filter::LevelFilter, fmt, prelude::*, EnvFilter};

use shadow_tls::{
    sip003::parse_sip003_options, RunningArgs, TlsAddrs, TlsExtConfig, TlsNames, V3Mode,
};

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about,
    long_about = "A proxy to expose real tls handshake to the firewall.\nGithub: github.com/ihciah/shadow-tls"
)]
struct Args {
    #[clap(subcommand)]
    cmd: Commands,
    #[clap(flatten)]
    opts: Opts,
}

#[derive(Parser, Debug, Default, Clone)]
struct Opts {
    #[clap(short, long, help = "Set parallelism manually")]
    threads: Option<u8>,
    #[clap(short, long, help = "Disable TCP_NODELAY")]
    disable_nodelay: bool,
    #[clap(long, help = "Use v3 protocol")]
    v3: bool,
    #[clap(long, help = "Strict mode(only for v3 protocol)")]
    strict: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[clap(about = "Run client side")]
    Client {
        #[clap(
            long = "listen",
            default_value = "[::1]:8080",
            help = "Shadow-tls client listen address(like \"[::1]:8080\")"
        )]
        listen: String,
        #[clap(
            long = "server",
            help = "Your shadow-tls server address(like \"1.2.3.4:443\")"
        )]
        server_addr: String,
        #[clap(
            long = "sni",
            help = "TLS handshake SNIs(like \"cloud.tencent.com\", \"captive.apple.com;cloud.tencent.com\")",
            value_parser = parse_client_names
        )]
        tls_names: TlsNames,
        #[clap(long = "password", help = "Password")]
        password: String,
        #[clap(
            long = "alpn",
            help = "Application-Layer Protocol Negotiation list(like \"http/1.1\", \"http/1.1;h2\")",
            value_delimiter = ';'
        )]
        alpn: Option<Vec<String>>,
    },
    #[clap(about = "Run server side")]
    Server {
        #[clap(
            long = "listen",
            default_value = "[::]:443",
            help = "Shadow-tls server listen address(like \"[::]:443\")"
        )]
        listen: String,
        #[clap(
            long = "server",
            help = "Your data server address(like \"127.0.0.1:8080\")"
        )]
        server_addr: String,
        #[clap(
            long = "tls",
            help = "TLS handshake server address(like \"cloud.tencent.com:443\", \"cloudflare.com:1.1.1.1:443;captive.apple.com;cloud.tencent.com\")",
            value_parser = parse_server_addrs
        )]
        tls_addr: TlsAddrs,
        #[clap(long = "password", help = "Password")]
        password: String,
    },
}

fn parse_client_names(addrs: &str) -> anyhow::Result<TlsNames> {
    TlsNames::try_from(addrs)
}

fn parse_server_addrs(arg: &str) -> anyhow::Result<TlsAddrs> {
    TlsAddrs::try_from(arg)
}

impl From<Args> for RunningArgs {
    fn from(args: Args) -> Self {
        let v3 = match (args.opts.v3, args.opts.strict) {
            (true, true) => V3Mode::Strict,
            (true, false) => V3Mode::Lossy,
            (false, _) => V3Mode::Disabled,
        };

        match args.cmd {
            Commands::Client {
                listen,
                server_addr,
                tls_names,
                password,
                alpn,
            } => Self::Client {
                listen_addr: listen,
                target_addr: server_addr,
                tls_names,
                tls_ext: TlsExtConfig::from(alpn),
                password,
                nodelay: !args.opts.disable_nodelay,
                v3,
            },
            Commands::Server {
                listen,
                server_addr,
                tls_addr,
                password,
            } => Self::Server {
                listen_addr: listen,
                target_addr: server_addr,
                tls_addr,
                password,
                nodelay: !args.opts.disable_nodelay,
                v3,
            },
        }
    }
}

// SIP003 [https://shadowsocks.org/en/wiki/Plugin.html](https://shadowsocks.org/en/wiki/Plugin.html)
pub(crate) fn get_sip003_arg() -> Option<Args> {
    macro_rules! env {
        ($key: expr) => {
            match std::env::var($key).ok() {
                None => return None,
                Some(val) if val.is_empty() => return None,
                Some(val) => val,
            }
        };
        ($key: expr, $fail_fn: expr) => {
            match std::env::var($key).ok() {
                None => return None,
                Some(val) if val.is_empty() => {
                    $fail_fn();
                    return None;
                }
                Some(val) => val,
            }
        };
    }

    let ss_remote_host = env!("SS_REMOTE_HOST");
    let ss_remote_port = env!("SS_REMOTE_PORT");
    let ss_local_host = env!("SS_LOCAL_HOST");
    let ss_local_port = env!("SS_LOCAL_PORT");
    let ss_plugin_options = env!("SS_PLUGIN_OPTIONS", || {
        tracing::error!("need SS_PLUGIN_OPTIONS when as SIP003 plugin");
        exit(-1);
    });

    let opts = parse_sip003_options(&ss_plugin_options).unwrap();
    let opts: HashMap<_, _> = opts.into_iter().collect();

    let threads = opts.get("threads").map(|s| s.parse::<u8>().unwrap());
    let v3 = opts.get("v3").is_some();
    let passwd = opts
        .get("passwd")
        .expect("need passwd param(like passwd=123456)");

    let args_opts = crate::Opts {
        threads,
        v3,
        ..Default::default()
    };
    let args = if opts.get("server").is_some() {
        let tls_addr = opts
            .get("tls")
            .expect("tls param must be specified(like tls=xxx.com:443)");
        let tls_addrs = parse_server_addrs(tls_addr)
            .expect("tls param parse failed(like tls=xxx.com:443 or tls=yyy.com:1.2.3.4:443;zzz.com:443;xxx.com)");
        Args {
            cmd: crate::Commands::Server {
                listen: format!("{ss_remote_host}:{ss_remote_port}"),
                server_addr: format!("{ss_local_host}:{ss_local_port}"),
                tls_addr: tls_addrs,
                password: passwd.to_owned(),
            },
            opts: args_opts,
        }
    } else {
        let host = opts
            .get("host")
            .expect("need host param(like host=www.baidu.com)");
        let hosts = parse_client_names(host).expect("tls names parse failed");
        Args {
            cmd: crate::Commands::Client {
                listen: format!("{ss_local_host}:{ss_local_port}"),
                server_addr: format!("{ss_remote_host}:{ss_remote_port}"),
                tls_names: hosts,
                password: passwd.to_owned(),
                alpn: Default::default(),
            },
            opts: args_opts,
        }
    };
    Some(args)
}

fn main() {
    std::env::set_var("MONOIO_FORCE_LEGACY_DRIVER", "1");
    println!(
        "test env: {:?}",
        std::env::var("MONOIO_FORCE_LEGACY_DRIVER")
    );
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy()
                .add_directive("rustls=off".parse().unwrap()),
        )
        .init();
    let args = get_sip003_arg().unwrap_or_else(Args::parse);
    let parallelism = get_parallelism(&args);
    let running_args = RunningArgs::from(args);
    tracing::info!("Start {parallelism}-thread {running_args}");
    if let Err(e) = ctrlc::set_handler(|| std::process::exit(0)) {
        tracing::error!("Unable to register signal handler: {e}");
    }
    let runnable = running_args.build().expect("unable to build runnable");
    runnable.start(parallelism).into_iter().for_each(|t| {
        if let Err(e) = t.join().expect("couldn't join on the associated thread") {
            tracing::error!("Thread exit: {e}");
        }
    });
}

fn get_parallelism(args: &Args) -> usize {
    if let Some(n) = args.opts.threads {
        return n as usize;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

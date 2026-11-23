//! A stand-in for Hamlib's rigctld in the supervisor's tests: takes rigctld's
//! flags, serves a `FakeRigctld` on `-T`/`-t` and says so on stderr.
//!
//! - `FAKE_RIGCTLD_FAIL=<text>`: print `<text>` to stderr and exit 1 at once,
//!   as rigctld does when it cannot open the rig.
//! - `FAKE_RIGCTLD_SPAM=<n>`: after the ready line, print `line 1` .. `line n`.
//! - `-l`: print a short model table in Hamlib's columns and exit.

use mercury_adapter::fake_rig::FakeRigctld;
use std::net::SocketAddr;

const MODELS: &str = " Rig #  Mfg                    Model                   Version         Status      Macro
     1  Hamlib                 Dummy                   20221128.0      Stable      RIG_MODEL_DUMMY
     2  Hamlib                 NET rigctl              20221111.0      Stable      RIG_MODEL_NETRIGCTL
  3073  Icom                   IC-7300                 20221227.10     Stable      RIG_MODEL_IC7300
  2028  Kenwood                TS-590SG                20221128.0      Stable      RIG_MODEL_TS590SG
";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Ok(text) = std::env::var("FAKE_RIGCTLD_FAIL") {
        eprintln!("{text}");
        std::process::exit(1);
    }
    let args: Vec<String> = std::env::args().collect();
    // `-l`: Hamlib's model table (a few rows), as rigctld prints it, then exit.
    if args.iter().any(|a| a == "-l") {
        print!("{MODELS}");
        return;
    }
    let flag = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let host = flag("-T").unwrap_or_else(|| "127.0.0.1".into());
    let port = flag("-t").unwrap_or_else(|| "4532".into());
    let addr: SocketAddr = format!("{host}:{port}").parse().expect("-T/-t");
    let _rig = FakeRigctld::start_at(addr).await;
    eprintln!("fake-rigctld: ready");
    if let Some(n) = std::env::var("FAKE_RIGCTLD_SPAM").ok().and_then(|n| n.parse::<u32>().ok()) {
        for i in 1..=n {
            eprintln!("line {i}");
        }
    }
    std::future::pending::<()>().await;
}

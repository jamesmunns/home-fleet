use surf;
use async_std::task;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum SubCommands {
    Plants {
        /// Server. Defaults to `goeff:8000`
        #[structopt(long = "host")]
        host: Option<String>,

        /// Relay number, 0..=3. Provide this or use `--all`.
        #[structopt(long = "relay")]
        relay: Option<usize>,

        /// Set relay(s) on
        #[structopt(long = "on")]
        on: bool,

        /// Set relay(s) off
        #[structopt(long = "off")]
        off: bool,

        /// Apply to all relays. Can't be used with `--relay`
        #[structopt(long = "all")]
        all: bool,

        // How many seconds? Defaults to 3600 (one hour)
        #[structopt(long = "for")]
        duration: Option<u64>,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    println!("Hello, world!");
    let opt = SubCommands::from_args();

    match opt {
        SubCommands::Plants{ host, all, relay, on, off, duration } => {
            let host: String = host.unwrap_or("http://goeff:8000".into());
            let duration = duration.unwrap_or(60 * 60);

            let setting = match (on, off) {
                (false, false) => {
                    return Err(format!("Set with `--on` or `--off`!").into());
                }
                (true, false) => true,
                (false, true) => false,
                (true, true) => {
                    return Err(format!("Can't set `--on` and `--off` at the same time!").into());
                }
            };

            let mut posts = vec![];

            match (all, relay) {
                (true, None) => {
                    for idx in 0..4 {
                        // /plant/<shelf>/force/<relay>/<setting>/<time_sec>
                        let uri = host.clone() + &format!(
                            "/plant/0/force/{}/{}/{}",
                            idx,
                            if setting { "on" } else { "off" },
                            duration,
                        );
                        posts.push(uri);
                    }
                }
                (true, Some(_)) => {
                    return Err(String::from("Can't use `--all` and `--relay` together!").into());
                }
                (false, None) => {
                    return Err(String::from("Use either `--all` or `--relay`!").into());
                }
                (false, Some(n)) => {
                    if n >= 4 {
                        return Err(String::from("Must select 0, 1, 2, or 3 for `--relay`!").into());
                    }

                    let uri = host.clone() + &format!(
                        "/plant/0/force/{}/{}/{}",
                        n,
                        if setting { "on" } else { "off" },
                        duration,
                    );
                    posts.push(uri);
                }
            };

            for post in posts {
                task::block_on(async {
                    let res: String = surf::post(post.as_str()).recv_string().await.unwrap();
                    println!("Server says: {}", res);
                });
            }


        }
    }
    Ok(())
}

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use feedr::{
    broker::{self, Client, Paths},
    mcp,
    model::*,
    ui,
};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Your agents' resources, within reach.")]
struct Cli {
    #[arg(long, global = true, env = "FEEDR_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    /// Configure opt-in mention discovery and feed completed hook text.
    Discovery {
        #[command(subcommand)]
        command: DiscoveryCommand,
    },
    /// Verify a PDF or video decoder without launching a desktop window.
    MediaCheck {
        resource: PathBuf,
    },
    /// Start the background service without opening a window.
    Start,
    /// Open the desktop feed (also the default command).
    Gui,
    /// Run the local background service in the foreground.
    Serve,
    /// Publish one or more file paths, directories or HTTP(S) URLs.
    Post {
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = "")]
        body: String,
        #[arg(value_name = "RESOURCE")]
        resources: Vec<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long,default_value="ready",value_parser=["creating","ready","failed"])]
        readiness: String,
        /// Read a PostInput object from a JSON file; '-' reads stdin.
        #[arg(long)]
        input: Option<String>,
    },
    /// Update a post using an UpdateInput JSON file ('-' for stdin).
    Update {
        #[arg(long)]
        input: String,
    },
    /// List cards as JSON.
    List {
        #[arg(long, default_value = "")]
        search: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        archived: bool,
        #[arg(long)]
        pinned: bool,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Show only currently available local resources and web links.
        #[arg(long)]
        available_only: bool,
    },
    Get {
        id: String,
    },
    /// List herdr sessions and their Feedr IDs as JSON.
    Sessions,
    /// Run an MCP server over stdio. Bind --session when launched globally.
    Mcp {
        #[arg(long)]
        session: Option<String>,
    },
    /// Print an MCP launch configuration using this executable.
    McpConfig {
        #[arg(long)]
        session: Option<String>,
    },
    /// Add a named/nonstandard local herdr socket to discovery.
    Connect {
        socket: PathBuf,
    },
    /// Check the local service and herdr connection status.
    Doctor,
    /// Create a labeled sample feed; optionally open it.
    Demo {
        #[arg(long)]
        no_launch: bool,
    },
}
#[derive(Subcommand)]
enum DiscoveryCommand {
    Status,
    Enable {
        #[arg(long)]
        session: Option<String>,
    },
    Disable,
    /// Replace discovery configuration with a JSON object ('-' reads stdin).
    Configure {
        #[arg(long)]
        input: String,
    },
    /// Extract mentions from completed assistant text or JSONL messages.
    Ingest {
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "-")]
        input: String,
        #[arg(long, default_value = "hook")]
        source: String,
        #[arg(long, default_value="plain",value_parser=["plain","codex","claude","jsonl"])]
        format: String,
    },
    /// Watch a log bound to an explicit session; new content only by default.
    WatchLog {
        #[arg(long)]
        session: String,
        #[arg(long, default_value="jsonl",value_parser=["plain","codex","claude","jsonl"])]
        format: String,
        #[arg(long)]
        from_start: bool,
        path: PathBuf,
    },
    UnwatchLog {
        path: PathBuf,
    },
}
fn discovery_command(client: &Client, command: DiscoveryCommand) -> Result<()> {
    use feedr::discovery::{Config, Ingested, Input, LogSource};
    let mut config: Config = client.call("discovery.config", json!({}))?;
    match command {
        DiscoveryCommand::Status => return print(config),
        DiscoveryCommand::Enable { session } => {
            config.enabled = true;
            config.sessions = session.map(|id| vec![id]);
        }
        DiscoveryCommand::Disable => config.enabled = false,
        DiscoveryCommand::Configure { input } => config = read_json(&input)?,
        DiscoveryCommand::WatchLog {
            session,
            format,
            path,
            from_start,
        } => {
            let path = std::path::absolute(path)?.to_string_lossy().into_owned();
            config
                .logs
                .retain(|log| log.path != path || log.session_id != session);
            config.logs.push(LogSource {
                path,
                session_id: session,
                format,
                from_start,
            });
        }
        DiscoveryCommand::UnwatchLog { path } => {
            let path = std::path::absolute(path)?.to_string_lossy().into_owned();
            config.logs.retain(|log| log.path != path);
        }
        DiscoveryCommand::Ingest {
            session,
            input,
            source,
            format,
        } => {
            let mut text = String::new();
            if input == "-" {
                std::io::stdin()
                    .take(128 * 1024 + 1)
                    .read_to_string(&mut text)?;
            } else {
                std::fs::File::open(input)?
                    .take(128 * 1024 + 1)
                    .read_to_string(&mut text)?;
            }
            anyhow::ensure!(text.len() <= 128 * 1024, "Mention input exceeds 128 KiB");
            let messages = if format == "plain" {
                vec![text]
            } else {
                text.lines()
                    .flat_map(|line| feedr::discovery::log_messages(line, &format))
                    .collect()
            };
            let mut result = Ingested::default();
            for text in messages {
                let mut ingested: Ingested = client.call(
                    "discovery.ingest",
                    serde_json::to_value(Input {
                        session_id: session.clone(),
                        text,
                        source: source.clone(),
                    })?,
                )?;
                result.created.append(&mut ingested.created);
                result.matched.append(&mut ingested.matched);
            }
            return print(result);
        }
    }
    print(client.call::<Config>("discovery.configure", serde_json::to_value(config)?)?)
}
fn media_check(resource: &std::path::Path) -> Result<()> {
    use feedr::media::{
        self,
        video::{Control, Player},
    };
    use std::{
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    let path = resource.canonicalize()?;
    let cancel = AtomicBool::new(false);
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
    {
        let count = media::pdf::pages(&path, &cancel)?;
        let first = media::pdf::render(&path, 1, 1000, &cancel)?;
        let last = media::pdf::render(&path, count, 1000, &cancel)?;
        return print(
            json!({"backend":"poppler", "pages":count,"first_page":first.size,"last_page":last.size}),
        );
    }
    let player = Player::new(&path, eframe::egui::Context::default());
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut first_frame = None;
    let status = loop {
        let (status, frame) = player.receive();
        if let Some(error) = status.error {
            anyhow::bail!("{error}");
        }
        if let Some(frame) = frame {
            first_frame = Some(frame.size);
        }
        if first_frame.is_some() && status.duration > 0. {
            break status;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Video did not produce a frame and duration"
        );
        std::thread::sleep(Duration::from_millis(30));
    };
    let seek = status.duration * 0.5;
    player.control(Control::Volume(0.));
    player.control(Control::Pause(false));
    player.control(Control::Seek(seek));
    let mut advanced = false;
    while Instant::now() < deadline {
        let (state, frame) = player.receive();
        if let Some(error) = state.error {
            anyhow::bail!("{error}");
        }
        if state.position >= seek - 0.2 && frame.is_some() {
            advanced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    anyhow::ensure!(advanced, "Video did not seek and render a new frame");
    player.control(Control::Pause(true));
    loop {
        let (state, _) = player.receive();
        if state.paused {
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "Video did not pause");
        std::thread::sleep(Duration::from_millis(30));
    }
    print(
        json!({"backend":"libmpv-software", "frame":first_frame,"duration":status.duration,"audio_track":status.audio,"seek_and_pause":advanced}),
    )
}
fn read_json<T: serde::de::DeserializeOwned>(file: &str) -> Result<T> {
    let mut text = String::new();
    if file == "-" {
        std::io::stdin().take(1_048_577).read_to_string(&mut text)?;
    } else {
        text = std::fs::read_to_string(file)?;
    }
    anyhow::ensure!(text.len() <= 1_048_576, "JSON input exceeds 1 MiB");
    Ok(serde_json::from_str(&text)?)
}
fn print(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Commands::MediaCheck { resource }) = &cli.command {
        return media_check(resource);
    }
    let paths = Paths::new(cli.data_dir)?;
    if matches!(cli.command, Some(Commands::Serve)) {
        return broker::serve(paths);
    }
    if let Some(Commands::McpConfig { session }) = &cli.command {
        let mut args = vec![
            "--data-dir".to_owned(),
            paths.root.to_string_lossy().into_owned(),
            "mcp".into(),
        ];
        if let Some(id) = session {
            args.extend(["--session".into(), id.clone()]);
        }
        return print(
            json!({"mcpServers":{"feedr":{"command":std::env::current_exe()?.to_string_lossy(),"args":args}}}),
        );
    }
    let client = Client::new(paths.clone());
    client.ensure_running()?;
    match cli.command.unwrap_or(Commands::Gui) {
        Commands::Discovery { command } => discovery_command(&client, command),
        Commands::Start => print(client.call::<Value>("ping", json!({}))?),
        Commands::MediaCheck { .. } => unreachable!(),
        Commands::Gui => ui::run(client),
        Commands::Post {
            title,
            body,
            resources,
            session,
            idempotency_key,
            readiness,
            input,
        } => {
            let mut post = if let Some(file) = input {
                read_json::<PostInput>(&file)?
            } else {
                PostInput {
                    title: title.context("Supply --title or --input")?,
                    body,
                    resources: resources
                        .into_iter()
                        .map(|uri| ResourceInput { uri, kind: None })
                        .collect(),
                    context: None,
                    session_id: session,
                    idempotency_key,
                    readiness,
                }
            };
            if post.context.is_none() {
                post.context = Some(Context::from_env());
            }
            print(client.call::<Post>("post", serde_json::to_value(post)?)?)
        }
        Commands::Update { input } => print(client.call::<Post>(
            "update",
            serde_json::to_value(read_json::<UpdateInput>(&input)?)?,
        )?),
        Commands::Get { id } => print(client.call::<Post>("get", json!({"id":id}))?),
        Commands::List {
            search,
            session,
            archived,
            pinned,
            limit,
            available_only,
        } => print(client.snapshot(&FeedQuery {
            search,
            session_id: session,
            archived,
            pinned_only: pinned,
            limit,
            available_only,
            ..Default::default()
        })?),
        Commands::Sessions => print(client.call::<Value>("sessions", json!({}))?),
        Commands::Mcp { session } => {
            tokio::runtime::Runtime::new()?.block_on(mcp::run(client, Context::from_env(), session))
        }
        Commands::Connect { socket } => {
            broker::add_socket(&paths, &socket)?;
            println!("Watching {}", socket.display());
            Ok(())
        }
        Commands::Doctor => {
            let snapshot = client.snapshot(&FeedQuery::default())?;
            print(
                json!({"version":env!("CARGO_PKG_VERSION"),"data_dir":paths.root,"socket":paths.socket,"posts":snapshot.total,"sessions":snapshot.sessions.len(),"herdr":snapshot.connections}),
            )
        }
        Commands::Demo { no_launch } => {
            seed_demo(&client)?;
            if no_launch { Ok(()) } else { ui::run(client) }
        }
        Commands::Serve | Commands::McpConfig { .. } => unreachable!(),
    }
}
fn seed_demo(client: &Client) -> Result<()> {
    let dir = client.paths.root.join("demo");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("welcome.md"),
        "# Your resources, within reach\n\nFeedr gives agent outputs a place to land.\n\n## Try the feed\n\n- Select an image to preview it.\n- Open a URL in your browser.\n- Reveal a file in Finder or your file manager.\n- Pin a card and filter by session.\n\n```sh\nfeedr post --title 'A useful result' ./report.md\n```\n\nThese are labeled demo posts, stored locally.\n",
    )?;
    let image = image::RgbImage::from_fn(960, 540, |x, y| {
        let r = 30 + (x * 45 / 960) as u8;
        let g = 55 + (y * 55 / 540) as u8;
        let b = 95 + ((x + y) * 75 / 1500) as u8;
        if (x / 80 + y / 80) % 5 == 0 {
            image::Rgb([r + 50, g + 40, b + 20])
        } else {
            image::Rgb([r, g, b])
        }
    });
    image.save(dir.join("study.png"))?;
    let context = Context {
        agent: "Feedr demo".into(),
        label: "Welcome to Feedr".into(),
        cwd: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    for (key, title, body, uris) in [
        (
            "demo-folder",
            "The workspace behind the result",
            "Jump straight to the files behind a deliverable.",
            vec![dir.to_string_lossy().into_owned()],
        ),
        (
            "demo-web",
            "Herdr's integration reference",
            "A useful link, ready to open in your default browser.",
            vec!["https://herdr.dev/docs/socket-api/".into()],
        ),
        (
            "demo-image",
            "A visual study is ready",
            "Select the image to inspect it inside Feedr.",
            vec!["study.png".into()],
        ),
        (
            "demo-markdown",
            "Everything an agent hands you, in one feed",
            "Open this Markdown guide, then publish a resource from a herdr pane.",
            vec!["welcome.md".into()],
        ),
    ] {
        let input = PostInput {
            title: title.into(),
            body: body.into(),
            resources: uris
                .into_iter()
                .map(|uri| ResourceInput { uri, kind: None })
                .collect(),
            context: Some(context.clone()),
            session_id: None,
            idempotency_key: Some(key.into()),
            readiness: "ready".into(),
        };
        let _: Post = client.call("post", serde_json::to_value(input)?)?;
    }
    Ok(())
}

extern crate structopt;
#[macro_use]
extern crate structopt_derive;

extern crate cargo_metadata;

#[macro_use] extern crate log;
extern crate simple_logger;

extern crate toml;

use std::process::{exit, Command, Stdio};
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::Read;
use std::borrow::Borrow;

use structopt::StructOpt;

use toml::Value;

#[derive(StructOpt, Debug)]
#[structopt(
    name = "cargo-remote",
    bin_name = "cargo",
)]
enum Opts {
    #[structopt(name = "remote")]
    Remote {
        #[structopt(short = "r", long = "remote", help = "remote ssh build server")]
        remote: Option<String>,

        #[structopt(
            short = "c",
            long = "copy-back",
            help = "transfer the target folder back to the local machine"
        )]
        copy_back: bool,

        #[structopt(
            long = "manifest-path",
            help = "Path to the manifest to execute",
            parse(from_os_str)
        )]
        manifest_path: Option<PathBuf>,

        #[structopt(
            short = "h",
            long = "transfer-hidden",
            help = "transfer hidden files and directories to the build server"
        )]
        hidden: bool,

        #[structopt(help = "cargo command that will be executed remotely")]
        command: String,

        #[structopt(
            help = "cargo options and flags that will be applied remotely",
            name = "remote options"
        )]
        options: Vec<String>,
    }
}

fn main() {
    simple_logger::init().unwrap();

    let Opts::Remote{
        remote,
        copy_back,
        manifest_path,
        hidden,
        command,
        options
    } = Opts::from_args();

    let manifest_path = manifest_path.as_ref().map(PathBuf::borrow);
    let project_metadata = cargo_metadata::metadata(manifest_path)
        .unwrap_or_else(|e| {
            error!("Could not read cargo metadata: {}", e);
            exit(-1);
        });

    // for now, assume that there is only one project and find it's root directory
    let (project_dir, project_name) = project_metadata.packages.first().map_or_else(|| {
        error!("No project found.");
        exit(-2);
    }, |project| {
        (
            Path::new(&project.manifest_path)
                .parent()
                .expect("Cargo.toml seems to have no parent directory?"),
            &project.name
        )
    });

    // TODO: refactor argument and config parsing and merging (related: kbknapp/clap-rs#748)
    let build_server = remote.unwrap_or_else(|| {
        let config_path = project_dir.join(".cargo-remote.toml");
        File::open(config_path).ok().and_then(|mut file| {
            let mut config_file_string = "".to_owned();
            // ignore the result for now, the whole config/argument parsing needs to
            // be refactored
            let _ = file.read_to_string(&mut config_file_string);
            config_file_string.parse::<Value>().ok()
        }).and_then(|value| {
            value["remote"].as_str().map(str::to_owned)
        }).unwrap_or_else(|| {
            error!("No remote build server was defined (use config file or --remote flag)");
            exit(-3);
        })
    });

    info!("Transferring sources to build server.");
    // transfer project to build server
    let mut rsync_to = Command::new("rsync");
    rsync_to.arg("-a".to_owned())
        .arg("--delete")
        .arg("--info=progress2")
        .arg("--exclude")
        .arg("target");

    if !hidden {
        rsync_to.arg("--exclude")
            .arg(".*");
    }

    rsync_to.arg("--rsync-path")
        .arg("mkdir -p remote-builds && rsync")
        .arg(format!("{}/", project_dir.to_string_lossy()))
        .arg(format!("{}:~/remote-builds/{}/", build_server, project_name))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| {
            error!("Failed to transfer project to build server (error: {})", e);
            exit(-4);
        });

    let build_command = format!(
        "cd ~/remote-builds/{}/; $HOME/.cargo/bin/cargo {} {}",
        project_name,
        command,
        options.iter().fold(
            String::new(),
            |acc, x| format!("{} {}", acc, x)
        )
    );

    info!("Starting build process.");
    Command::new("ssh")
        .arg(&build_server)
        .arg(build_command)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| {
            error!("Failed to run cargo command remotely (error: {})", e);
            exit(-5);
        });

    if copy_back {
        info!("Transferring artifacts back to client.");
        Command::new("rsync")
            .arg("-a")
            .arg("--delete")
            .arg("--info=progress2")
            .arg(format!("{}:~/remote-builds/{}/target/", build_server, project_name))
            .arg(format!("{}/target/", project_dir.to_string_lossy()))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit())
            .output()
            .unwrap_or_else(|e| {
                error!("Failed to transfer target back to local machine (error: {})", e);
                exit(-6);
            });
    }
}

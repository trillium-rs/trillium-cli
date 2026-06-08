use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use trillium_grpc_codegen::{Options, generate_from_proto};

/// Which halves of the service to generate.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Emit {
    /// the client and the server (the default)
    #[default]
    Both,
    /// only the client: the `<Service>Client` struct and its call methods
    Client,
    /// only the server: the service trait and the `<Service>Server<T>` handler
    Server,
}

/// Generate Rust modules from a .proto service definition.
///
/// Produces one .rs file per .proto package, written into the output
/// directory. Each file contains the prost-generated message types plus
/// the trillium-grpc service trait and `Server<T>` Handler. Output is
/// formatted with prettyplease and intended to be committed.
#[derive(Parser, Debug)]
pub struct GrpcCli {
    /// path to the .proto file to compile
    proto: PathBuf,

    /// directory to write generated .rs files into (created if missing)
    #[arg(default_value = "./src")]
    out: PathBuf,

    /// additional include path for resolving `import` statements
    ///
    /// the parent directory of the .proto is included automatically
    #[arg(short = 'I', long = "include")]
    includes: Vec<PathBuf>,

    /// which halves to generate: `both`, `client`, or `server`
    ///
    /// Generate only the half you need: `client` for a crate that calls the
    /// service, `server` for one that implements it. Defaults to both.
    #[arg(long, value_enum, default_value_t = Emit::Both)]
    emit: Emit,
}

impl GrpcCli {
    pub fn run(self) {
        let mut includes = self.includes;
        if let Some(parent) = self.proto.parent()
            && !parent.as_os_str().is_empty()
            && !includes.iter().any(|p| p == parent)
        {
            includes.push(parent.to_path_buf());
        }

        let opts = Options {
            include_paths: includes,
            client: self.emit != Emit::Server,
            server: self.emit != Emit::Client,
            ..Options::default()
        };

        let generated = match generate_from_proto(&[&self.proto], &opts) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("codegen failed: {e}");
                std::process::exit(1);
            }
        };

        if let Err(e) = std::fs::create_dir_all(&self.out) {
            eprintln!(
                "could not create output directory {}: {e}",
                self.out.display()
            );
            std::process::exit(1);
        }

        for (rel_path, content) in &generated.files {
            let path = self.out.join(rel_path);
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("could not write {}: {e}", path.display());
                std::process::exit(1);
            }
            println!("wrote {}", path.display());
        }
    }
}

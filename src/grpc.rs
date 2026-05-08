use clap::Parser;
use std::path::PathBuf;
use trillium_grpc_codegen::{Options, generate_from_proto};

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

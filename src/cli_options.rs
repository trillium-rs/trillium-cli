use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub enum Cli {
    /// Static file server and reverse proxy
    Serve(crate::StaticCli),

    #[cfg(unix)]
    /// Development server for trillium applications
    DevServer(crate::DevServer),

    /// Make http requests using the trillium client
    Client(crate::ClientCli),
}

impl Cli {
    pub fn run(self) {
        use Cli::*;
        match self {
            Serve(s) => s.run(),
            #[cfg(unix)]
            DevServer(d) => d.run(),
            Client(c) => c.run(),
        }
    }
}

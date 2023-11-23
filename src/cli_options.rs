use clap::Parser;

#[derive(Parser, Debug)]
pub enum Cli {
    /// Static file server
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

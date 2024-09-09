use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(short, long)]
    pub id: usize,

    #[arg(long, default_value = "master.ron")]
    pub config_file: String,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_rest_server: bool,
}

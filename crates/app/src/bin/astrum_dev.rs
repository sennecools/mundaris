fn main() {
    if let Err(error) = astrum_app::developer_bridge::cli_main() {
        eprintln!("astrum_dev: {error:#}");
        std::process::exit(1);
    }
}

fn main() {
    if let Err(error) = mundaris_app::developer_bridge::cli_main() {
        eprintln!("mundaris_dev: {error:#}");
        std::process::exit(1);
    }
}

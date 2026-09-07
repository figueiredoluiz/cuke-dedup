#![forbid(unsafe_code)]

fn main() {
    match cuke_dedup::cli::run_cli() {
        Ok(code) => std::process::exit(code),
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
            }) =>
        {
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(2);
        }
    }
}

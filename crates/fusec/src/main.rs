fn main() {
    std::process::exit(fusec::cli::run(std::env::args().skip(1)));
}

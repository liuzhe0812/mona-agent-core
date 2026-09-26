fn main() {
    std::process::exit(sandbox::runner::run(std::env::args_os().skip(1).collect()));
}

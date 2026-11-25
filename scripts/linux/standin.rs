// A stand-in C compiler and archiver for `cargo check` of a Linux target on Windows: nothing is linked, so it only
// has to leave a file where one is asked for, and say it is a GCC that understands every flag.
use std::{env, fs, path::Path};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let touch = |p: &str| { if let Some(d) = Path::new(p).parent() { let _ = fs::create_dir_all(d); } let _ = fs::write(p, b"!<arch>\n"); };
    if args.iter().any(|a| a == "--version" || a == "-v" || a == "-V") { println!("gcc (stand-in) 12.2.0"); return; }
    if args.iter().any(|a| a == "-dumpmachine") { println!("{}", env::var("STANDIN_TRIPLE").unwrap_or_else(|_| "x86_64-linux-gnu".into())); return; }
    if args.iter().any(|a| a == "-dumpversion") { println!("12.2.0"); return; }
    if let Some(i) = args.iter().position(|a| a == "-o") { if let Some(p) = args.get(i + 1) { touch(p); } return; }
    if args.iter().any(|a| a == "-E") { println!("gcc\n"); println!("#pragma message GCC"); return; }
    for a in &args { if let Some(p) = a.strip_prefix("-o") { touch(p); } if a.ends_with(".a") { touch(a); } }
}

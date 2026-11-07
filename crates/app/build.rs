// The About page shows the commit the app was built from, or "dev" outside a Git checkout (a
// release tarball).
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "dev".into());
    println!("cargo:rustc-env=AMP_GIT_COMMIT={commit}");
    // HEAD changes on checkout; the branch's ref changes on commit.
    let mut watched = vec![git(&["rev-parse", "--git-path", "HEAD"])];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(git(&["rev-parse", "--git-path", &branch]));
    }
    // A path that does not exist (a packed ref) would rerun this script on every build.
    for path in watched.into_iter().flatten().filter(|p| std::path::Path::new(p).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }
}

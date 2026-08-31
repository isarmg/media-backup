use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=MEDIA_BACKUP_SOURCE_REVISION");
    let revision =
        env::var("MEDIA_BACKUP_SOURCE_REVISION").unwrap_or_else(|_| "unversioned".to_owned());
    if revision != "unversioned"
        && (revision.len() != 40
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        panic!("MEDIA_BACKUP_SOURCE_REVISION must be 40 lowercase hexadecimal characters");
    }
    println!("cargo:rustc-env=MEDIA_BACKUP_SOURCE_REVISION={revision}");
    println!(
        "cargo:rustc-env=MEDIA_BACKUP_BUILD_TARGET={}",
        env::var("TARGET").expect("Cargo provides TARGET to build scripts")
    );
}

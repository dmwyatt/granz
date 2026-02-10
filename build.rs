fn main() {
    println!("cargo:rerun-if-env-changed=GRANS_BUILD_DATE");
    println!("cargo:rerun-if-env-changed=GRANS_BUILD_SHA");

    let version = match (
        std::env::var("GRANS_BUILD_DATE").ok(),
        std::env::var("GRANS_BUILD_SHA").ok(),
    ) {
        (Some(date), Some(sha)) => format!("{date} ({sha})"),
        _ => "dev".to_string(),
    };

    println!("cargo:rustc-env=GRANS_VERSION={version}");
}

//! What a v1 import would bring over, without any key or secret (the same summary the app shows).
fn main() {
    let Some(folder) = anytask_migrate::v1_folder() else { return println!("no v1 data folder") };
    let summary = anytask_migrate::read(&folder).summary();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
}

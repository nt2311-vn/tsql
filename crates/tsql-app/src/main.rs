use tsql_core::ProjectInfo;

fn main() {
    let info = ProjectInfo::default();
    println!("{} {}", info.name, info.version);
}

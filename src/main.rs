use filesystem::filesystem::create_disk;

fn main() {
    let _ = create_disk("test.img",134217728,16384);
}

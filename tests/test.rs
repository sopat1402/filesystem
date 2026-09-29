use filesystem::constants::*;
use filesystem::directories::{delete, resolve_path};
use filesystem::file_errors::FileError;
use filesystem::files::File as FsFile;
use filesystem::filesystem::Filesystem;

#[test]
fn test_read_write() -> Result<(), FileError> {
    let fs = Filesystem::open(String::from("test.img"))?;
    let name = format!("read_write_{}", std::process::id());
    let path = format!("/{name}");
    println!("\n===== read/write test: {path} =====");
    if resolve_path(&fs.disk, path.clone(), ROOT_INODE_NUM).is_ok() {
        println!("[setup] found leftover file from a previous run, deleting it");
        delete(&fs.disk, ROOT_INODE_NUM, name.clone())?;
    }
    println!("\n-- step 1: create file and write initial contents --");
    let mut file = FsFile::open(&fs, &path, ROOT_INODE_NUM, O_CREAT | O_EXCL | O_RDWR)?;
    let payload_size = BLOCK_SIZE - BLOCK_HEADER_SIZE;
    let mut expected = Vec::with_capacity(payload_size + 32);
    for i in 0..payload_size + 32 {
        expected.push((i % 251) as u8);
    }
    println!(
        "writing {} bytes ({} bytes past a single block's payload of {})",
        expected.len(),
        expected.len() - payload_size,
        payload_size
    );
    let written = file.write(&expected, 0)?;
    assert_eq!(written, expected.len());
    println!("wrote {written} bytes — OK, spans two blocks on disk");
    println!("\n-- step 2: read the file back from offset 0 --");
    file.fseek(0)?;
    let read_back = file.read(expected.len())?;
    assert_eq!(read_back, expected);
    println!("read {} bytes back, matches what was written — OK", read_back.len());
    println!("\n-- step 3: overwrite bytes across the block boundary --");
    let overwrite_offset = payload_size - 4;
    let replacement = b"UPDATED";
    println!(
        "overwriting {} bytes at offset {} (block boundary is at {})",
        replacement.len(),
        overwrite_offset,
        payload_size
    );
    expected[overwrite_offset..overwrite_offset + replacement.len()].copy_from_slice(replacement);
    file.fseek(overwrite_offset as u32)?;
    let written = file.write(replacement, 0)?;
    assert_eq!(written, replacement.len());
    println!("overwrote {written} bytes in place — OK, file size unchanged");
    println!("\n-- step 4: read the file back again to confirm the overwrite --");
    file.fseek(0)?;
    let read_back = file.read(expected.len())?;
    assert_eq!(read_back, expected);
    println!("read {} bytes back, overwrite is correctly in place — OK", read_back.len());
    println!("\n-- step 5: reopen with O_APPEND and append one byte --");
    let mut append_file = FsFile::open(&fs, &path, ROOT_INODE_NUM, O_APPEND | O_RDWR)?;
    let suffix = b"!";
    let written = append_file.write(suffix, 0)?;
    assert_eq!(written, suffix.len());
    expected.extend_from_slice(suffix);
    println!("appended {written} byte(s) — file should now be {} bytes total", expected.len());
    println!("\n-- step 6: final read-back --");
    append_file.fseek(0)?;
    let read_back = append_file.read(expected.len())?;
    assert_eq!(read_back, expected);
    println!("read {} bytes back, matches expected final contents — OK", read_back.len());
    assert!(resolve_path(&fs.disk, path, ROOT_INODE_NUM).is_ok());
    println!("\n===== read/write test passed =====\n");
    Ok(())
}

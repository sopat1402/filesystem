use filesystem::constants::*;
use filesystem::directories::{delete, resolve_path};
use filesystem::file_errors::FileError;
use filesystem::files::File as FsFile;
use std::fs::File;

#[test]
fn test_read_write() -> Result<(), FileError> {
    let disk = File::options()
        .read(true)
        .write(true)
        .open("test.img")
        .map_err(|_| FileError::ReadError)?;
    let name = format!("read_write_{}", std::process::id());
    let path = format!("/{name}");
    if resolve_path(&disk, path.clone(), ROOT_INODE_NUM).is_ok() {
        delete(&disk, ROOT_INODE_NUM, name.clone())?;
    }
    let mut file = FsFile::open(
        &disk,
        &path,
        ROOT_INODE_NUM,
        O_CREAT | O_EXCL | O_RDWR,
    )?;
    let payload_size = BLOCK_SIZE - BLOCK_HEADER_SIZE;
    let mut expected = Vec::with_capacity(payload_size + 32);
    for i in 0..payload_size + 32 {
        expected.push((i % 251) as u8);
    }
    assert_eq!(file.write(&disk, &expected, 0)?, expected.len());
    file.fseek(&disk, 0)?;
    assert_eq!(file.read(&disk, expected.len())?, expected);
    let overwrite_offset = payload_size - 4;
    let replacement = b"UPDATED";
    expected[overwrite_offset..overwrite_offset + replacement.len()]
        .copy_from_slice(replacement);
    file.fseek(&disk, overwrite_offset as u32)?;
    assert_eq!(file.write(&disk, replacement, 0)?, replacement.len());
    file.fseek(&disk, 0)?;
    assert_eq!(file.read(&disk, expected.len())?, expected);
    let mut append_file =
        FsFile::open(&disk, &path, ROOT_INODE_NUM, O_APPEND | O_RDWR)?;
    let suffix = b"!";
    assert_eq!(append_file.write(&disk, suffix, 0)?, suffix.len());
    expected.extend_from_slice(suffix);
    append_file.fseek(&disk, 0)?;
    assert_eq!(append_file.read(&disk, expected.len())?, expected);
    assert!(resolve_path(&disk, path, ROOT_INODE_NUM).is_ok());
    Ok(())
}

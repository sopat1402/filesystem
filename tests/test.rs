use filesystem::directories::*;
use std::fs::File;
use filesystem::constants::*;
use filesystem::file_errors::FileError;

#[test]
fn test()->Result<(),FileError>{
    let disk=File::options()
        .read(true)
        .write(true)
        .open("test.img").map_err(|_| FileError::ReadError)?;
    println!("---- Filesystem Test ----\n");
    println!("====");
    println!("Intial read of root");
    print_dir(&disk,ROOT_INODE_NUM)?;
    println!("====");
    println!("Making a directory called testicle");
    let new_dir=make_dir(&disk,ROOT_INODE_NUM,String::from("testicle"),0,0,S_IRGRP)?;
    print_dir(&disk,ROOT_INODE_NUM)?;
    println!("====");
    println!("Making directory spermatocyte in testicle");
    let parent=resolve_path(&disk,String::from("/testicle"),0)?;
    assert_eq!(parent,new_dir);
    make_dir(&disk,parent,String::from("spermatocyte"),0,0,S_IWGRP)?;
    println!("====");
    println!("Testing directory creation in testicle");
    let id=resolve_path(&disk,String::from("/testicle"),0)?;
    print_dir(&disk,id)?;
    println!("====");
    println!("\n---- Filesystem test completed ----\n");
    Ok(())
}

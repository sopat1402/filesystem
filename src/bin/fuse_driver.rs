use filesystem::file_errors::FileError;
use filesystem::filesystem::Filesystem;
use filesystem::directories::delete;
use filesystem::inode::{find_inode,write_inode};
use std::os::fd::RawFd;


#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseSetattrIn {
    valid: u32,
    padding: u32,
    fh: u64,
    size: u64,
    lock_owner: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
    atimensec: u32,
    mtimensec: u32,
    ctimensec: u32,
    mode: u32,
    unused4: u32,
    uid: u32,
    gid: u32,
}
const _: () = assert!(std::mem::size_of::<FuseSetattrIn>() == 88);

#[repr(C)]
#[derive(Clone, Copy)]
struct FuseWriteIn {
    fh: u64,
    offset: u64,
    size: u32,
    write_flags: u32,
    lock_owner: u64,
    flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FuseWriteOut {
    size: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseEntryOut {
    nodeid: u64,
    generation: u64,
    entry_valid: u64,
    attr_valid: u64,
    entry_valid_nsec: u32,
    attr_valid_nsec: u32,
    attr: FuseAttr,
}
const _: () = assert!(std::mem::size_of::<FuseEntryOut>() == 128);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseAttr {
    ino: u64,
    size: u64,
    blocks: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
    atimensec: u32,
    mtimensec: u32,
    ctimensec: u32,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
    blksize: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseGetattrIn {
    getattr_flags: u32,
    dummy: u32,
    fh: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseCreateIn {
    flags: u32,
    mode: u32,
    umask: u32,
    open_flags: u32,
}
const _: () = assert!(std::mem::size_of::<FuseCreateIn>() == 16);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseOpenIn {
    flags: u32,
    open_flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseOpenOut {
    fh: u64,
    open_flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseReadIn {
    fh: u64,
    offset: u64,
    size: u32,
    read_flags: u32,
    lock_owner: u64,
    flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseDirent {
    ino: u64,
    off: u64,
    namelen: u32,
    typ: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseReleaseDirIn {
    fh: u64,
    flags: u32,
    release_flags: u32,
    lock_owner: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseAttrOut {
    attr_valid: u64,
    attr_valid_nsec: u32,
    dummy: u32,
    attr: FuseAttr,
}

const _: () = assert!(std::mem::size_of::<FuseAttr>() == 88);
const _: () = assert!(std::mem::size_of::<FuseGetattrIn>() == 16);
const _: () = assert!(std::mem::size_of::<FuseOpenIn>() == 8);
const _: () = assert!(std::mem::size_of::<FuseOpenOut>() == 16);
const _: () = assert!(std::mem::size_of::<FuseReadIn>() == 40);
const _: () = assert!(std::mem::size_of::<FuseDirent>() == 24);
const _: () = assert!(std::mem::size_of::<FuseReleaseDirIn>() == 24);
const _: () = assert!(std::mem::size_of::<FuseAttrOut>() == 104);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseInHeader {
    len: u32,
    opcode: u32,
    unique: u64,
    nodeid: u64,
    uid: u32,
    gid: u32,
    pid: u32,
    total_extlen: u16,
    padding: u16,
}
const _: () = assert!(std::mem::size_of::<FuseInHeader>() == 40);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseOutHeader {
    len: u32,
    error: i32,
    unique: u64,
}
const _: () = assert!(std::mem::size_of::<FuseOutHeader>() == 16);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseInitIn {
    major: u32,
    minor: u32,
    max_readahead: u32,
    flags: u32,
    flags2: u32,
    unused: [u32; 11],
}
const _: () = assert!(std::mem::size_of::<FuseInitIn>() == 64);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FuseInitOut {
    major: u32,
    minor: u32,
    max_readahead: u32,
    flags: u32,
    max_background: u16,
    congestion_threshold: u16,
    max_write: u32,
    time_gran: u32,
    max_pages: u16,
    map_alignment: u16,
    flags2: u32,
    max_stack_depth: u32,
    unused: [u32; 6],
}
const _: () = assert!(std::mem::size_of::<FuseInitOut>() == 64);

unsafe fn read_struct<T: Copy>(buf: &[u8]) -> Option<T> {
    if buf.len() < std::mem::size_of::<T>() {
        return None;
    }

    Some(unsafe {
        std::ptr::read_unaligned(buf.as_ptr() as *const T)
    })
}

fn as_bytes<T: Copy>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            value as *const T as *const u8,
            std::mem::size_of::<T>(),
        )
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn split_request(buf: &[u8], bytes_read: usize) -> Result<(FuseInHeader, Vec<u8>), FileError> {
    if bytes_read < std::mem::size_of::<FuseInHeader>() || buf.len() < bytes_read {
        return Err(FileError::InvalidRequest);
    }
    let header = unsafe {
        read_struct::<FuseInHeader>(&buf[..bytes_read])
    }
    .ok_or(FileError::InvalidRequest)?;
    if header.len as usize != bytes_read {
        return Err(FileError::InvalidRequest);
    }
    let body = buf[std::mem::size_of::<FuseInHeader>()..bytes_read].to_vec();
    Ok((header, body))
}

fn build_reply(unique: u64, errno: i32, body: &[u8]) -> Vec<u8> {
    let header = FuseOutHeader {
        len: (std::mem::size_of::<FuseOutHeader>() + body.len()) as u32,
        error: -errno,
        unique,
    };
    let mut reply = Vec::with_capacity(header.len as usize);
    reply.extend_from_slice(as_bytes(&header));
    reply.extend_from_slice(body);
    reply
}

fn get_fuse_fd(mountpoint: &str) -> Result<RawFd, FileError> {
    let mut sockets = [0; 2];
    let ret = unsafe {
        libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sockets.as_mut_ptr())
    };
    if ret < 0 {
        return Err(FileError::OpenError);
    }
    let flags = unsafe { libc::fcntl(sockets[1], libc::F_GETFD) };
    if flags < 0 {
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
        }
        return Err(FileError::OpenError);
    }
    let ret = unsafe {
        libc::fcntl(sockets[1], libc::F_SETFD, flags & !libc::FD_CLOEXEC)
    };
    if ret < 0 {
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
        }
        return Err(FileError::OpenError);
    }
    let _child = match std::process::Command::new("fusermount3")
        .arg("-o")
        .arg("fsname=myfs,default_permissions")
        .arg(mountpoint)
        .env("_FUSE_COMMFD", sockets[1].to_string())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            unsafe {
                libc::close(sockets[0]);
                libc::close(sockets[1]);
            }
            return Err(FileError::OpenError);
        }
    };
    unsafe {
        libc::close(sockets[1]);
    }
    let mut data = [0u8; 1];
    let cmsg_space = unsafe {
        libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) as usize
    };
    let mut control = vec![0u8; cmsg_space];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = control.len();
    let ret = unsafe { libc::recvmsg(sockets[0], &mut msg, 0) };
    if ret < 0 {
        unsafe { libc::close(sockets[0]); }
        return Err(FileError::ReadError);
    }
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if cmsg.is_null() {
        unsafe { libc::close(sockets[0]); }
        return Err(FileError::InvalidRequest);
    }
    let cmsg_ref = unsafe { &*cmsg };
    if cmsg_ref.cmsg_level != libc::SOL_SOCKET || cmsg_ref.cmsg_type != libc::SCM_RIGHTS {
        unsafe { libc::close(sockets[0]); }
        return Err(FileError::InvalidRequest);
    }
    let fuse_fd = unsafe {
        *(libc::CMSG_DATA(cmsg) as *const libc::c_int)
    };
    unsafe {
        libc::close(sockets[0]);
    }
    Ok(fuse_fd)
}

fn handle_init(_header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    let init_in = unsafe {
        read_struct::<FuseInitIn>(body)
    }
    .ok_or(FileError::InvalidRequest)?;
    if init_in.major != 7 {
        return Err(FileError::Unsupported);
    }
    let init_out = FuseInitOut {
        major: 7,
        minor: 31,
        max_readahead: init_in.max_readahead,
        flags: 0,
        max_background: 0,
        congestion_threshold: 0,
        max_write: 4096,
        time_gran: 1,
        max_pages: 0,
        map_alignment: 0,
        flags2: 0,
        max_stack_depth: 0,
        unused: [0; 6],
    };
    Ok(as_bytes(&init_out).to_vec())
}

fn handle_lookup(fs: &Filesystem, header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    if body.len()==0 || body[body.len()-1]!=b'\0'{
        return Err(FileError::InvalidRequest);
    }
    let name = String::from_utf8(body[..body.len() - 1].to_vec()).map_err(|_| FileError::InvalidRequest)?;
    let parent = header.nodeid - 1;
    let stat = fs.lookup(parent, &name)?;
    let entry_out = FuseEntryOut {
        generation: stat.generation as u64,
        nodeid: stat.ino + 1,
        entry_valid: 1,
        attr_valid: 1,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: FuseAttr {
            ino: stat.ino + 1,
            size: stat.size,
            blocks: stat.blocks * 8,
            atime: stat.atime,
            mtime: stat.mtime,
            ctime: stat.ctime,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: stat.mode as u32,
            nlink: stat.nlink as u32,
            uid: stat.uid,
            gid: stat.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        },
    };
    Ok(as_bytes(&entry_out).to_vec())
}

fn handle_getattr(fs: &Filesystem, header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    let _getattr_in = unsafe {
        read_struct::<FuseGetattrIn>(body)
    }.ok_or(FileError::InvalidRequest)?;
    let ino = header.nodeid - 1;
    let attrs = fs.stat(ino)?;
    let fuse_attr = FuseAttr {
        ino: attrs.ino + 1,
        size: attrs.size,
        blocks: attrs.blocks * 8,
        atime: attrs.atime,
        mtime: attrs.mtime,
        ctime: attrs.ctime,
        atimensec: 0,
        mtimensec: 0,
        ctimensec: 0,
        mode: attrs.mode as u32,
        nlink: attrs.nlink as u32,
        uid: attrs.uid,
        gid: attrs.gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    };
    let reply = FuseAttrOut {
        attr_valid: 1,
        attr_valid_nsec: 0,
        dummy: 0,
        attr: fuse_attr,
    };
    Ok(as_bytes(&reply).to_vec())
}

fn handle_opendir(fs: &Filesystem,header: &FuseInHeader,body: &[u8]) -> Result<Vec<u8>, FileError> {
    let _open_in =
        unsafe { read_struct::<FuseOpenIn>(body) }.ok_or(FileError::InvalidRequest)?;
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    fs.read_dir(ino)?;

    let open_out = FuseOpenOut {
        fh: 0,
        open_flags: 0,
        padding: 0,
    };
    Ok(as_bytes(&open_out).to_vec())
}

fn fuse_dirent_type(mode: u16) -> u32 {
    match mode & 0o170000 {
        0o010000 => 1,  // FIFO
        0o020000 => 2,  // character device
        0o040000 => 4,  // directory
        0o060000 => 6,  // block device
        0o100000 => 8,  // regular file
        0o120000 => 10, // symbolic link
        0o140000 => 12, // socket
        _ => 0,          // unknown
    }
}

fn handle_readdir(fs: &Filesystem,header: &FuseInHeader,body: &[u8]) -> Result<Vec<u8>, FileError> {
    let read_in =
        unsafe { read_struct::<FuseReadIn>(body) }.ok_or(FileError::InvalidRequest)?;
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    let entries = fs.read_dir(ino)?;
    let start = usize::try_from(read_in.offset)
        .unwrap_or(usize::MAX)
        .min(entries.len());
    let max_size = read_in.size as usize;
    let mut reply = Vec::with_capacity(max_size.min(8192));

    for (index, (name, child_ino)) in entries.iter().enumerate().skip(start) {
        let name_len = u32::try_from(name.len()).map_err(|_| FileError::NameTooLong)?;
        let raw_len = std::mem::size_of::<FuseDirent>()
            .checked_add(name.len())
            .ok_or(FileError::EOverflow)?;
        let padded_len = raw_len
            .checked_add(7)
            .map(|len| len & !7)
            .ok_or(FileError::EOverflow)?;
        if reply.len().saturating_add(padded_len) > max_size {
            break;
        }

        let child = fs.stat(*child_ino)?;
        let dirent = FuseDirent {
            ino: child.ino + 1,
            off: (index as u64) + 1,
            namelen: name_len,
            typ: fuse_dirent_type(child.mode),
        };
        reply.extend_from_slice(as_bytes(&dirent));
        reply.extend_from_slice(name.as_bytes());
        reply.resize(reply.len() + padded_len - raw_len, 0);
    }

    Ok(reply)
}

fn handle_releasedir(body: &[u8]) -> Result<Vec<u8>, FileError> {
    let _release_in =
        unsafe { read_struct::<FuseReleaseDirIn>(body) }.ok_or(FileError::InvalidRequest)?;
    Ok(Vec::new())
}

fn handle_read(fs:&Filesystem,header:&FuseInHeader,body:&[u8])->Result<Vec<u8>,FileError>{
    let read_in =unsafe { 
        read_struct::<FuseReadIn>(body) 
    }.ok_or(FileError::InvalidRequest)?;
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    let buf=fs.read_at(ino,read_in.offset,read_in.size as usize)?;
    Ok(buf)
}

fn handle_write(fs: &Filesystem,header: &FuseInHeader,body: &[u8]) -> Result<Vec<u8>, FileError> {
    let write_in =unsafe {
        read_struct::<FuseWriteIn>(body)
    }.ok_or(FileError::InvalidRequest)?;
    let data_start = std::mem::size_of::<FuseWriteIn>();
    let data_len =usize::try_from(write_in.size).map_err(|_| FileError::EOverflow)?;
    let data_end = data_start.checked_add(data_len).ok_or(FileError::EOverflow)?;
    if data_end > body.len() {
        return Err(FileError::InvalidRequest);
    }
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    let written = fs.write_at(ino, write_in.offset, &body[data_start..data_end])?;
    let out = FuseWriteOut {
        size: u32::try_from(written).map_err(|_| FileError::EOverflow)?,
        padding: 0,
    };
    Ok(as_bytes(&out).to_vec())
}

fn handle_create(fs: &Filesystem,header: &FuseInHeader,body: &[u8]) -> Result<Vec<u8>, FileError> {
    let create_in =unsafe { 
        read_struct::<FuseCreateIn>(body) 
    }.ok_or(FileError::InvalidRequest)?;
    let name_start = std::mem::size_of::<FuseCreateIn>();
    if body.len() <= name_start || body.last() != Some(&0) {
        return Err(FileError::InvalidRequest);
    }
    let name_bytes = &body[name_start..body.len() - 1];
    if name_bytes.is_empty() || name_bytes.contains(&0) || name_bytes.contains(&b'/') {
        return Err(FileError::InvalidRequest);
    }
    let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| FileError::InvalidRequest)?;
    let parent_ino = header
        .nodeid
        .checked_sub(1)
        .ok_or(FileError::InvalidRequest)?;
    let parent = fs.stat(parent_ino)?;
    if parent.mode & 0o170000 != 0o040000 {
        return Err(FileError::NotDirectory);
    }
    let permissions = (create_in.mode & 0o7777) & !(create_in.umask & 0o7777);
    let mode = u16::try_from(0o100000u32 | permissions).map_err(|_| FileError::InvalidRequest)?;
    let ino = filesystem::directories::add_dirent(
        &fs.disk,
        parent_ino,
        name,
        mode,
        header.uid,
        header.gid,
    )?;
    let stat = fs.stat(ino)?;
    let entry_out = FuseEntryOut {
        nodeid: stat.ino + 1,
        generation: stat.generation as u64,
        entry_valid: 1,
        attr_valid: 1,
        entry_valid_nsec: 0,
        attr_valid_nsec: 0,
        attr: FuseAttr {
            ino: stat.ino + 1,
            size: stat.size,
            blocks: stat.blocks * 8,
            atime: stat.atime,
            mtime: stat.mtime,
            ctime: stat.ctime,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: stat.mode as u32,
            nlink: stat.nlink as u32,
            uid: stat.uid,
            gid: stat.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        },
    };
    let open_out = FuseOpenOut {
        fh: 0,
        open_flags: 0,
        padding: 0,
    };
    let mut reply = Vec::with_capacity(
        std::mem::size_of::<FuseEntryOut>() + std::mem::size_of::<FuseOpenOut>(),
    );
    reply.extend_from_slice(as_bytes(&entry_out));
    reply.extend_from_slice(as_bytes(&open_out));
    Ok(reply)
}

fn handle_setattr(fs: &Filesystem, header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    const MODE: u32 = 1 << 0;
    const ATIME: u32 = 1 << 4;
    const MTIME: u32 = 1 << 5;
    const FH: u32 = 1 << 6;
    const ATIME_NOW: u32 = 1 << 7;
    const MTIME_NOW: u32 = 1 << 8;
    const CTIME: u32 = 1 << 10;
    const KILL_SUIDGID: u32 = 1 << 11;
    const KILL_SUID: u32 = 1 << 12;
    const KILL_SGID: u32 = 1 << 13;
    const TIMES_SET: u32 = 1 << 15;
    const FATTR_SIZE: u32 = 1 << 3;
    const FATTR_OPEN: u32 = 1 << 14;
    const FATTR_LOCKOWNER: u32 = 1 << 9;
    const SUPPORTED: u32 = MODE | FATTR_SIZE | FATTR_LOCKOWNER | FATTR_OPEN
        | ATIME | MTIME | FH | ATIME_NOW | MTIME_NOW | CTIME
        | KILL_SUIDGID | KILL_SUID | KILL_SGID | TIMES_SET;
    let input = unsafe { read_struct::<FuseSetattrIn>(body) }.ok_or(FileError::InvalidRequest)?;
    println!("{} {} {}",input.valid,input.size,input.valid&!SUPPORTED);
    if input.valid & !SUPPORTED != 0 {
        return Err(FileError::Unsupported);
    }
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    let mut inode = find_inode(&fs.disk, ino)?;
    if header.uid != 0 && header.uid != inode.i_uid {
        return Err(FileError::PermissionDenied);
    }
    let now = now_secs();
    if input.valid & MODE != 0 {
        inode.i_mode = (inode.i_mode & !0o7777) | (input.mode as u16 & 0o7777);
    }
    if input.valid & ATIME_NOW != 0 {
        inode.i_atime = now;
    } else if input.valid & ATIME != 0 {
        inode.i_atime = input.atime;
    }
    if input.valid & MTIME_NOW != 0 {
        inode.i_mtime = now;
    } else if input.valid & MTIME != 0 {
        inode.i_mtime = input.mtime;
    }
    if input.valid & KILL_SUIDGID != 0 {
        inode.i_mode &= !0o6000;
    } else {
        if input.valid & KILL_SUID != 0 {
            inode.i_mode &= !0o4000;
        }
        if input.valid & KILL_SGID != 0 {
            inode.i_mode &= !0o2000;
        }
    }
    if input.valid & CTIME != 0 {
        inode.i_ctime = input.ctime;
    } else if input.valid & (MODE | ATIME | MTIME | ATIME_NOW | MTIME_NOW | KILL_SUIDGID | KILL_SUID | KILL_SGID) != 0 {
        inode.i_ctime = now;
    }
    if input.valid & FATTR_SIZE != 0 {
        if input.size != 0 {
            return Err(FileError::Unsupported);
        }
        if inode.i_mode & 0o170000 != 0o100000 {
            return Err(FileError::NotFile);
        }
        filesystem::files::truncate_to_zero(&fs.disk, ino)?;
        inode = find_inode(&fs.disk, ino)?;
    }
    write_inode(&fs.disk, ino, &inode.serialise())?;
    let attrs = fs.stat(ino)?;
    let reply = FuseAttrOut {
        attr_valid: 1,
        attr_valid_nsec: 0,
        dummy: 0,
        attr: FuseAttr {
            ino: attrs.ino + 1,
            size: attrs.size,
            blocks: attrs.blocks * 8,
            atime: attrs.atime,
            mtime: attrs.mtime,
            ctime: attrs.ctime,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: attrs.mode as u32,
            nlink: attrs.nlink as u32,
            uid: attrs.uid,
            gid: attrs.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        },
    };
    Ok(as_bytes(&reply).to_vec())
}

fn handle_open(fs: &Filesystem, header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    let open_in = unsafe { read_struct::<FuseOpenIn>(body) }.ok_or(FileError::InvalidRequest)?;
    let ino = header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    let access = open_in.flags & libc::O_ACCMODE as u32;
    if access == libc::O_ACCMODE as u32 {
        return Err(FileError::InvalidFlags);
    }
    let attrs = fs.stat(ino)?;
    if attrs.mode & 0o170000 == 0o040000 {
        return Err(FileError::NotFile);
    }
    let open_out = FuseOpenOut {
        fh: access as u64,
        open_flags: 0,
        padding: 0,
    };
    Ok(as_bytes(&open_out).to_vec())
}

fn handle_release(_body:&[u8])->Result<Vec<u8>,FileError>{
    Ok(Vec::new())
}

fn handle_delete(fs: &Filesystem,header:&FuseInHeader,body: &[u8])->Result<Vec<u8>,FileError>{
    let parent_inode=header.nodeid.checked_sub(1).ok_or(FileError::InvalidRequest)?;
    if body.last() != Some(&0){
        return Err(FileError::InvalidRequest);
    }
    let name_bytes = &body[0..body.len() - 1];
    if name_bytes.is_empty() || name_bytes.contains(&0) || name_bytes.contains(&b'/') {
        return Err(FileError::InvalidRequest);
    }
    let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| FileError::InvalidRequest)?;
    delete(&fs.disk,parent_inode,name)?;
    Ok(Vec::new())
}

fn dispatch_request(fs: &Filesystem, header: &FuseInHeader, body: &[u8]) -> Result<Vec<u8>, FileError> {
    match header.opcode {
        1   => handle_lookup(fs, header, body),
        3   => handle_getattr(fs, header, body),
        4   => handle_setattr(fs,header,body),
        10  => handle_delete(fs,header,body),
        11  => handle_delete(fs,header,body),
        14  => handle_open(fs,header,body),
        15  => handle_read(fs,header,body),
        16  => handle_write(fs,header,body),
        18  => handle_release(body),
        26  => handle_init(header, body),
        27  => handle_opendir(fs, header, body),
        28  => handle_readdir(fs, header, body),
        29  => handle_releasedir(body),
        35  => handle_create(fs,header,body),
        _   => Err(FileError::Unsupported),
    }
}

fn error_to_errno(error: FileError) -> i32 {
    match error {
        FileError::NameNotFound => libc::ENOENT,
        FileError::NameExists => libc::EEXIST,
        FileError::NotDirectory => libc::ENOTDIR,
        FileError::NotFile => libc::EISDIR,
        FileError::PermissionDenied => libc::EACCES,
        FileError::InvalidFlags => libc::EINVAL,
        FileError::NameTooLong => libc::ENAMETOOLONG,
        FileError::NoMoreBlocks | FileError::NoInodes => libc::ENOSPC,
        FileError::Unsupported => libc::ENOSYS,
        FileError::InvalidRequest => libc::EINVAL,
        FileError::EOverflow => libc::EOVERFLOW,
        FileError::OpenError | FileError::ReadError | FileError::WriteError => libc::EIO,
        FileError::CorruptedINode | FileError::CorruptedBlock => libc::EIO,
        FileError::MisalignedSize => libc::EINVAL,
    }
}

fn main() -> Result<(), FileError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <image> <mountpoint>", args[0]);
        return Err(FileError::InvalidRequest);
    }
    let image = &args[1];
    let mountpoint = &args[2];
    let fs = Filesystem::open(image.clone())?;
    let fuse_fd = get_fuse_fd(mountpoint)?;
    let mut buf = vec![0u8; 8192];
    loop {
        let bytes_read = unsafe {
            libc::read(
                fuse_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if bytes_read <= 0 {
            unsafe { libc::close(fuse_fd); }
            return Err(FileError::ReadError);
        }
        let (header, body) = split_request(&buf, bytes_read as usize)?;
        println!("opcode : {}",header.opcode);
        if header.opcode == 2 || header.opcode == 42{
            continue;
        }
        let (errno, response_body) = match dispatch_request(&fs, &header, &body) {
            Ok(response_body) => (0, response_body),
            Err(error) => {
                println!("error : {error}");
                (error_to_errno(error), Vec::new())
            },
        };
        let reply = build_reply(header.unique, errno, &response_body);
        let written = unsafe {
            libc::write(
                fuse_fd,
                reply.as_ptr() as *const libc::c_void,
                reply.len(),
            )
        };
        if written != reply.len() as isize {
            unsafe { libc::close(fuse_fd); }
            return Err(FileError::WriteError);
        }
    }
}

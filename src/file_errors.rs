use std::fmt;

#[derive(Debug)]
pub enum FileError{
	OpenError,
	WriteError,
    ReadError,
    CorruptedINode,
    CorruptedBlock,
    NoInodes,
    NotDirectory,
    NameExists,
    NoMoreBlocks,
    NameNotFound,
    Overflow,
    NotFile,
    MisalignedSize,
    PermissionDenied,
    InvalidFlags,
    Unsupported,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            FileError::OpenError=> "Error opening file.",
            FileError::WriteError=> "Error writing to file.",
            FileError::ReadError=>"Error reading from file",
            FileError::CorruptedINode=>"Corrupted INode",
            FileError::CorruptedBlock=>"Corrupted block",
            FileError::NoInodes=>"Out of inodes",
            FileError::NotDirectory=>"No such directory",
            FileError::NameExists=>"Entity with that name exists",
            FileError::NoMoreBlocks=>"Out of blocks",
            FileError::NameNotFound=>"No such name found",
            FileError::Overflow=>"Offset beyond file size",
            FileError::NotFile=>"Not a file",
            FileError::MisalignedSize=>"Provided disk size is not block aligned",
            FileError::PermissionDenied=>"Permission Denied",
            FileError::InvalidFlags=>"Invalid file flags",
            FileError::Unsupported=>"Unsupported operation",
        };

        write!(f, "{message}")
    }
}

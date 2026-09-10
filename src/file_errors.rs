use std::fmt;

pub enum FileError{
	OpenError,
	WriteError,
    ReadError,
    CorruptedINode,
    CorruptedBlock,
    NoInodes,
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
        };

        write!(f, "{message}")
    }
}

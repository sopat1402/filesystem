
const CRC_TABLE:[u32;256]={
    let mut table=[0u32;256];
    let mut i=0;
    while i<256{
        let mut crc=i as u32;
        let mut j=0;
        while j<8{
            if crc&1!=0{
                crc=(crc>>1)^0xEDB88320;
            }else{
                crc>>=1;
            }
            j+=1;
        }
        table[i]=crc;
        i+=1;
    }
    table
};

pub fn crc32(data:&[u8])->u32{
    let mut crc=0xFFFFFFFFu32;
    for &byte in data{
        let table_index=((crc^byte as u32)&0xFF) as usize;
        crc=(crc>>8)^CRC_TABLE[table_index];
    }
    !crc
}

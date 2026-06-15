use std::io::{Cursor, Seek, SeekFrom, Write};

fn main() {
    let mut c = Cursor::new(Vec::<u8>::new());
    c.seek(SeekFrom::Start(5)).unwrap();
    println!(
        "After seek(5): pos={}, len={}",
        c.position(),
        c.get_ref().len()
    );
    c.write_all(&[255]).unwrap();
    println!(
        "After write 1 byte: pos={}, len={}",
        c.position(),
        c.get_ref().len()
    );
    println!("Bytes: {:?}", c.get_ref());
}

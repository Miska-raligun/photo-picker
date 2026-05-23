use std::env;
use std::fs::File;
use std::io::BufReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).expect("usage: exif_debug <file.jpg>");
    let file = File::open(&path)?;
    let mut reader = BufReader::new(file);
    let exif_reader = exif::Reader::new();
    let exif_data = exif_reader.read_from_container(&mut reader)?;
    for f in exif_data.fields() {
        println!("{:?} (ifd={:?}): {}", f.tag, f.ifd_num, f.display_value().with_unit(&exif_data));
    }
    Ok(())
}

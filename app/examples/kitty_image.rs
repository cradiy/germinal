use std::io::{self, Write};

fn main() -> io::Result<()> {
    // 4x4 RGBA checkerboard, encoded as a Kitty Graphics Protocol direct payload.
    const RGBA_BASE64: &str =
        "/0BA/0CA////QED/QID//0CA////QED/QID///9AQP//QED/QID///9AQP9AgP//QID///9AQP9AgP///0BA/w==";

    let mut stdout = io::stdout().lock();
    write!(
        stdout,
        "\x1b_Ga=T,f=32,s=4,v=4,i=1,c=12,r=6,C=1;{RGBA_BASE64}\x1b\\"
    )?;
    stdout.flush()
}

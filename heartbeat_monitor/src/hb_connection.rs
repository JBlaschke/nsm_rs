use std::io::Read;
use std::net::TcpStream;

pub fn stream_read(stream: &mut TcpStream) -> std::io::Result<String>{
    let mut buf = [0; 1024];
    let mut message = String::new();

    loop {

        let bytes_read = match stream.read(&mut buf) {
            Ok(n) => n,
            // Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => 0,
            Err(err) => { return Err(err); }
        };
        let s = std::str::from_utf8(&buf[..bytes_read]).unwrap();
        message.push_str(s);

        if bytes_read < buf.len() {
            break;
        }
    }

    Ok(message)
}
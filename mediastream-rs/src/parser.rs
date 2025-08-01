use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::{self, BufRead},
    mem::swap,
};

use lazy_static::lazy_static;
use regex::Regex;
use smol_str::SmolStr;

use crate::format::{M3uMedia, M3uPlaylist, directives};

lazy_static! {
    /// From `https://github.com/Raiper34/m3u-parser-generator/blob/c8e479161dcc4ec3d5490631fa42a1647741481d/src/m3u-parser.ts#L52` (Modified)
    static ref ATTRIBUTE_REGEX: Regex = Regex::new("([^ ]*?)=\"(.*?)\"").expect("Regular expression error");
}

fn parse_attributes(input: impl AsRef<str>) -> HashMap<SmolStr, SmolStr> {
    let mut result = HashMap::new();
    for (_, [key, value]) in ATTRIBUTE_REGEX
        .captures_iter(input.as_ref())
        .map(|x| x.extract())
    {
        result.insert(key.into(), value.into());
    }

    result
}

/// A parser to parse M3U/M3U8 file.
///
/// Example:
/// ```rust
/// use mediastream_rs::Parser;
/// use std::io::Cursor;
///
/// let mut parser = Parser::new(Cursor::new(r#"
/// #EXTM3U x-tvg-url="test"
/// #EXTINF:1 tvg-id="a" provider-type="iptv",A
/// http://example.com/A.m3u8"#));
/// parser.parse().unwrap();
/// let _result = parser.get_playlist();
/// ```
pub struct Parser<T: BufRead> {
    reader: T,
    buffer: String,
    playlist: M3uPlaylist,
    media: M3uMedia,
}

impl<T: BufRead> Parser<T> {
    /// Create a parser from a stream (`BufRead + Seek + 'static`)
    ///
    /// Example:
    /// ```rust
    /// use mediastream_rs::Parser;
    /// use std::io::Cursor;
    ///
    /// let mut parser = Parser::new(Cursor::new(r#"
    /// #EXTM3U x-tvg-url="test"
    /// #EXTINF:1 tvg-id="a" provider-type="iptv",A
    /// http://example.com/A.m3u8"#));
    /// ```
    pub fn new(reader: T) -> Self {
        Self {
            reader,
            buffer: String::new(),
            playlist: M3uPlaylist::default(),
            media: M3uMedia::default(),
        }
    }

    /// Parse the content from the stream until EOF, and return the error if occurred
    ///
    /// Example:
    /// ```rust
    /// use mediastream_rs::Parser;
    /// use std::io::Cursor;
    ///
    /// let mut parser = Parser::new(Cursor::new(r#"
    /// #EXTM3U x-tvg-url="test"
    /// #EXTINF:1 tvg-id="a" provider-type="iptv",A
    /// http://example.com/A.m3u8"#));
    /// parser.parse().unwrap();
    /// ```
    pub fn parse(&mut self) -> Result<(), ParseError> {
        self.parse_m3u_header()?;

        while let Some(line) = self.next_line()? {
            if line.starts_with('#') {
                // directive
                self.parse_directive(line)?;
            } else {
                // media
                self.media.location = SmolStr::new(line);
                let mut media = M3uMedia::default();
                swap(&mut self.media, &mut media);
                self.playlist.medias.push(media);
            }
        }

        Ok(())
    }

    /// Get the parsed `M3uPlaylist`, and you can continue the next parsing
    ///
    /// Example:
    /// ```rust
    /// use mediastream_rs::Parser;
    /// use std::io::Cursor;
    ///
    /// let mut parser = Parser::new(Cursor::new(r#"
    /// #EXTM3U x-tvg-url="test"
    /// #EXTINF:1 tvg-id="a" provider-type="iptv",A
    /// http://example.com/A.m3u8"#));
    /// parser.parse().unwrap();
    /// _ = parser.get_playlist();
    /// ```
    pub fn get_playlist(&mut self) -> M3uPlaylist {
        let mut result = M3uPlaylist::default();
        swap(&mut self.playlist, &mut result);
        result
    }

    /// Return the inner reader
    ///
    /// Example:
    /// ```
    /// use mediastream_rs::Parser;
    /// use std::io::Cursor;
    ///
    /// let mut parser = Parser::new(Cursor::new(r#"
    /// #EXTM3U x-tvg-url="test"
    /// #EXTINF:1 tvg-id="a" provider-type="iptv",A
    /// http://example.com/A.m3u8"#));
    ///
    /// let _the_cursor = parser.into_inner();
    /// ```
    pub fn into_inner(self) -> T {
        self.reader
    }

    fn next_line(&mut self) -> Result<Option<String>, io::Error> {
        loop {
            self.buffer.clear();
            match self.reader.read_line(&mut self.buffer) {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                Err(e) => return Err(e),
            }

            if self.buffer.trim().len() != 0 {
                return Ok(Some(self.buffer.trim().to_owned()));
            }
        }
    }

    fn parse_m3u_header(&mut self) -> Result<(), ParseError> {
        let first_line = self.next_line()?.ok_or(ParseError::UnexpectedEOF)?;

        if !first_line.starts_with(directives::EXTM3U) {
            return Err(ParseError::NotAPlaylist);
        }

        let attributes = first_line
            .chars()
            .skip(directives::EXTM3U_LEN)
            .skip_while(|x| x.is_whitespace())
            .collect::<String>();

        let attributes = parse_attributes(attributes);
        self.playlist.attributes.extend(attributes);

        Ok(())
    }

    fn parse_media_info(&mut self, value: SmolStr) -> Result<(), ParseError> {
        let mut splited_value = value.split(',');
        // parse duration with attributes
        let maybe_duration = splited_value.next().ok_or(ParseError::MissingDuration)?;

        // parse title
        let title = splited_value.next();

        // parse name
        self.media.name = title.map(|x| x.into());
        let mut splited_duration = maybe_duration.splitn(2, ' ');

        // parse duration
        let duration = splited_duration.next().ok_or(ParseError::MissingDuration)?;
        self.media.duration = duration.parse().map_err(|_| ParseError::MissingDuration)?;

        // parse attribute
        if let Some(attributes) = splited_duration.next() {
            self.media.attributes.extend(parse_attributes(attributes));
        }

        Ok(())
    }

    fn parse_directive(&mut self, line: String) -> Result<(), ParseError> {
        let mut splited_line = line.splitn(2, ':');
        let key = splited_line.next().unwrap().into();
        let value = splited_line.next().map(|x| x.into());

        if key == directives::EXTINF {
            self.parse_media_info(value.unwrap_or_default())?;
        } else if key == directives::PLAYLIST {
            self.playlist.title = Some(value.unwrap_or_default());
        } else {
            self.media.extension_data.insert(key, value);
        }

        Ok(())
    }
}

/// Error occurred during parsing
#[derive(Debug)]
pub enum ParseError {
    /// File doesn't start with `#EXTM3U`
    NotAPlaylist,
    /// `#EXTINF:<duration>`, the duration is missing
    MissingDuration,
    /// Unexpected EOF while parsing
    UnexpectedEOF,
    // IO error
    IoError(io::Error),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            Self::NotAPlaylist => write!(f, "Not a playlist file"),
            Self::IoError(e) => e.fmt(f),
            Self::UnexpectedEOF => write!(f, "Unexpected EOF"),
            Self::MissingDuration => write!(f, "Duration of a media is missing"),
        }
    }
}
impl Error for ParseError {}
impl From<io::Error> for ParseError {
    fn from(value: io::Error) -> Self {
        Self::IoError(value)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::{Parser, parser::parse_attributes};

    #[test]
    fn test_parse_attributes() {
        let result = parse_attributes("HELLO=\"WORLD\" FOO=\"BAR\"");
        assert!(result.contains_key("HELLO"));
        assert_eq!(result.get("FOO").unwrap(), "BAR");
        assert!(!result.contains_key("NOT_FOUND"));
    }

    #[test]
    fn test_parse_stream() {
        let data = r"
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-MEDIA-SEQUENCE:8885
#EXT-X-DISCONTINUITY-SEQUENCE:0
#EXT-X-TARGETDURATION:6
#EXT-X-INDEPENDENT-SEGMENTS
#EXTINF:6.00000000,
21-35-08882.html
#EXTINF:6.00000000,
21-35-08883.html
#EXTINF:6.00000000,
21-35-08884.html";
        let mut parser = Parser::new(Cursor::new(data));
        parser.parse().unwrap();
        let result = parser.get_playlist();
        assert_eq!(result.medias.len(), 3);
    }

    #[test]
    fn test_parse_list() {
        let data = r#"
#EXTM3U x-tvg-url="test"

#EXTINF:1 tvg-id="a" provider-type="iptv",A
http://example.com/A.m3u8

#EXTINF:2 tvg-id="b" provider-type="iptv",B
http://example.com/C.m3u8

#EXTINF:3 tvg-id="c" provider-type="iptv",C
http://example.com/C.m3u8

#EXTINF:4 tvg-id="d" provider-type="iptv",D
http://example.com/D.m3u8
"#;
        let mut parser = Parser::new(Cursor::new(data));
        parser.parse().unwrap();
        let result = parser.get_playlist();

        assert_eq!(result.attributes.get("x-tvg-url").unwrap(), "test");
        assert_eq!(result.medias.len(), 4);
        assert_eq!(result.medias.get(1).unwrap().name.as_ref().unwrap(), "B");
        assert_eq!(
            result
                .medias
                .get(2)
                .unwrap()
                .attributes
                .get("provider-type")
                .unwrap(),
            "iptv"
        );
        assert_eq!(
            result.medias.get(3).unwrap().location,
            "http://example.com/D.m3u8"
        );
    }
}

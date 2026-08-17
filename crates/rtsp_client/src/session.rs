use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::info;
use url::Url;

pub struct RtspSession {
    url: Url,
    stream: TcpStream,
    cseq: usize,
    buffer: BytesMut,
    options: RtspOptions,
    describe: RtspDescribe,
    tracks: Vec<SdpTrack>,
    setups: Vec<RtspSetup>,
}

impl RtspSession {
    pub fn new(url: Url, stream: TcpStream) -> Self {
        Self {
            url,
            stream,
            cseq: 1,
            buffer: BytesMut::with_capacity(8192),
            options: RtspOptions::default(),
            describe: RtspDescribe::default(),
            tracks: Vec::new(),
            setups: Vec::new(),
        }
    }

    pub async fn handshake(&mut self) -> Result<()> {
        self.handshake_options().await?;
        self.handshake_describe().await?;
        self.handshake_setup().await?;
        Ok(())
    }

    async fn handshake_options(&mut self) -> Result<()> {
        let request = format!(
            "OPTIONS {} RTSP/1.0\r\nCSeq: {}\r\n\r\n",
            self.url.path(),
            self.cseq
        );
        self.stream.write_all(request.as_bytes()).await?;

        let header_bytes = self.parse_header().await?;
        let header_text = std::str::from_utf8(&header_bytes).context("Invalid header")?;

        let lines = header_text.lines();

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim().to_ascii_lowercase();
                let val = val.trim();

                match key.as_str() {
                    "public" => {
                        for method in val.split(',') {
                            self.options.public.insert(method.trim().to_uppercase());
                        }
                    }
                    _ => {}
                }
            }
        }

        info!("Handshake OPTIONS completed : {:?}", self.options);
        self.cseq += 1;
        Ok(())
    }

    async fn handshake_describe(&mut self) -> Result<()> {
        if !self.options.public.contains("DESCRIBE") {
            bail!("Server does not support DESCRIBE");
        }
        let request = format!(
            "DESCRIBE {} RTSP/1.0\r\nCSeq: {}\r\nAccept: application/sdp\r\n\r\n",
            self.url.path(),
            self.cseq
        );
        self.stream.write_all(request.as_bytes()).await?;

        let header_bytes = self.parse_header().await?;
        let header_text = std::str::from_utf8(&header_bytes).context("Invalid header")?;

        let lines = header_text.lines();

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim().to_ascii_lowercase();
                let val = val.trim();

                match key.as_str() {
                    "content-length" => {
                        self.describe.content_length = val.parse()?;
                    }
                    _ => {}
                }
            }
        }

        info!("Handshake DESCRIBE completed : {:?}", self.describe);

        let body_bytes = self.buffer.split_to(self.describe.content_length);
        let body_text = std::str::from_utf8(&body_bytes).context("Invalid body")?;

        let lines = body_text.lines();

        let mut current_track: Option<SdpTrack> = None;

        for line in lines {
            let line = line.trim();
            if line.len() < 2 || &line[1..2] != "=" {
                continue;
            }
            let key = &line[0..1];
            let val = &line[2..];

            match key {
                "m" => {
                    if let Some(track) = current_track.take() {
                        self.tracks.push(track);
                    }
                    let mut parts = val.split_whitespace();
                    let media_type = parts.next().unwrap_or("").to_string();
                    let _port = parts.next();
                    let _proto = parts.next();
                    let payload_type = parts.next().unwrap_or("0").parse().unwrap_or(0);

                    current_track = Some(SdpTrack {
                        media_type,
                        payload_type,
                        ..Default::default()
                    });
                }
                "a" => {
                    if let Some(ref mut track) = current_track {
                        if let Some((key, val)) = val.split_once(':') {
                            match key {
                                "control" => {
                                    track.control_url = val.to_string();
                                }
                                "rtpmap" => {
                                    if let Some((pt_str, codec_info)) = val.split_once(' ') {
                                        if let Ok(pt) = pt_str.trim().parse::<u8>() {
                                            if pt == track.payload_type {
                                                track.codec = codec_info
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or("")
                                                    .to_uppercase();
                                            }
                                        }
                                    }
                                }
                                "fmtp" => {
                                    if let Some((pt_str, fmt_info)) = val.split_once(' ') {
                                        if let Ok(pt) = pt_str.trim().parse::<u8>() {
                                            if pt == track.payload_type {
                                                fmt_info.split(';').for_each(|fmt| {
                                                    if let Some((key, val)) = fmt.split_once('=') {
                                                        match key {
                                                            "packetization-mode" => {
                                                                track.packetization_mode =
                                                                    val.parse().unwrap_or(0);
                                                            }
                                                            "sprop-parameter-sets" => {
                                                                track.sps_pps = val.to_string();
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(track) = current_track {
            self.tracks.push(track);
        }

        self.cseq += 1;
        Ok(())
    }

    async fn handshake_setup(&mut self) -> Result<()> {
        if !self.options.public.contains("SETUP") {
            bail!("Server does not support SETUP");
        }

        for (idx, track) in self.tracks.clone().iter().enumerate() {
            let request = format!(
                "SETUP {}/{} RTSP/1.0\r\nCSeq: {}\r\nTransport: RTP/AVP/TCP;unicast;interleaved={}-{}\r\n\r\n",
                self.url.path(),
                track.control_url,
                self.cseq,
                idx * 2,
                idx * 2 + 1
            );
            self.stream.write_all(request.as_bytes()).await?;

            let header_bytes = self.parse_header().await?;
            let header_text = std::str::from_utf8(&header_bytes).context("Invalid header")?;

            let lines = header_text.lines();
            let mut setup = RtspSetup::default();

            for line in lines {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if let Some((key, val)) = line.split_once(':') {
                    let key = key.trim().to_ascii_lowercase();
                    let val = val.trim();

                    match key.as_str() {
                        "session" => {
                            if let Some((session_id, _)) = val.trim().split_once(';') {
                                setup.session_id = session_id.to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }

            self.setups.push(setup);

            info!("Handshake SETUP completed : {:?}", self.setups);
            self.cseq += 1;
        }

        Ok(())
    }

    pub async fn play(&mut self) -> Result<()> {
        if !self.options.public.contains("PLAY") {
            bail!("Server does not support PLAY");
        }

        let request = format!(
            "PLAY {} RTSP/1.0\r\nCSeq: {}\r\nSession: {}\r\nRange: npt=0.000-\r\n\r\n",
            self.url.path(),
            self.cseq,
            self.setups[0].session_id
        );
        self.stream.write_all(request.as_bytes()).await?;

        let header_bytes = self.parse_header().await?;
        let header_text = std::str::from_utf8(&header_bytes).context("Invalid header")?;

        Ok(())
    }

    async fn parse_header(&mut self) -> Result<BytesMut> {
        let header_end_pos = loop {
            if let Some(pos) = self.buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
            if self.buffer.len() > 8192 {
                bail!("Too long header");
            }
            let n = self.stream.read_buf(&mut self.buffer).await?;
            if n == 0 {
                bail!("Connection closed");
            }
        };
        Ok(self.buffer.split_to(header_end_pos + 4))
    }

    pub async fn receive(&mut self) -> Result<()> {
        let mut buf = [0u8; 8192];
        loop {
            let n = self.stream.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            println!("received {} bytes", n);
            let data = &buf[..n];
        }

        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct RtspOptions {
    pub public: HashSet<String>,
}

#[derive(Default, Debug)]
pub struct RtspDescribe {
    pub content_length: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SdpTrack {
    pub media_type: String,
    pub payload_type: u8,
    pub codec: String,
    pub control_url: String,
    pub packetization_mode: u8,
    pub sps_pps: String,
}

#[derive(Default, Debug)]
pub struct RtspSetup {
    pub session_id: String,
}

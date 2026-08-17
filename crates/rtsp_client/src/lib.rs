use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tracing::info;
use url::Url;

use crate::session::RtspSession;

pub mod session;

pub struct RtspClient {
    pub session: RtspSession,
}

impl RtspClient {
    pub async fn connect(addr: &str) -> Result<Self> {
        let url = Url::parse(addr)?;
        let stream = TcpStream::connect(format!(
            "{}:{}",
            url.host_str().context("invalid host")?,
            url.port().context("invalid port")?
        ))
        .await?;
        info!("Connected to {}", url);

        let mut session = RtspSession::new(url, stream);
        session.handshake().await?;

        Ok(Self { session })
    }

    pub async fn play(&mut self) -> Result<()> {
        self.session.play().await?;
        self.session.receive().await?;
        Ok(())
    }
    
}

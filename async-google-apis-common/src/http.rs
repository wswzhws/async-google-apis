use crate::*;
use anyhow::Context;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use tokio::io::AsyncSeekExt;

pub trait AsyncWriteUnpin: tokio::io::AsyncWrite + std::marker::Unpin + Send + Sync {}

impl<T> AsyncWriteUnpin for T where T: tokio::io::AsyncWrite + std::marker::Unpin + Send + Sync {}

fn body_to_str(b: hyper::body::Bytes) -> String {
    String::from_utf8(b.to_vec()).unwrap_or("[UTF-8 decode failed]".into())
}

/// This type is used as type parameter to the following functions, when `rq` is `None`.
#[derive(Debug, Serialize)]
pub struct EmptyRequest {}

/// This type is used as type parameter for when no response is expected.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct EmptyResponse {}

/// Result of a method that can (but doesn't always) download data.
#[derive(Debug, PartialEq)]
pub enum DownloadResult<T: DeserializeOwned + std::fmt::Debug> {
    /// Downloaded data has been written to the supplied Writer.
    Downloaded,
    /// A structured response has been returned.
    Response(T),
}

/// The Content-Type header is set automatically to application/json.
pub async fn do_request<
    Req: Serialize + std::fmt::Debug,
    Resp: DeserializeOwned + Clone + Default,
    Cli,
>(
    cl: &TlsClient<Cli, Full<Bytes>>,
    path: &str,
    headers: &[(hyper::header::HeaderName, String)],
    http_method: &str,
    rq: Option<Req>,
) -> Result<Resp>
where
    Cli: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Cli::Future: Unpin + Send,
    Cli::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Cli::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Cli::Response: hyper_util::client::legacy::connect::Connection,
{
    use futures::future::FutureExt;
    do_request_with_headers(cl, path, headers, http_method, rq)
        .map(|r| r.map(|t| t.0))
        .await
}

/// The Content-Type header is set automatically to application/json. Also returns response
/// headers.
pub async fn do_request_with_headers<
    Req: Serialize + std::fmt::Debug,
    Resp: DeserializeOwned + Clone + Default,
    Cli,
>(
    cl: &TlsClient<Cli, Full<Bytes>>,
    path: &str,
    headers: &[(hyper::header::HeaderName, String)],
    http_method: &str,
    rq: Option<Req>,
) -> Result<(Resp, hyper::HeaderMap)>
where
    Cli: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Cli::Future: Unpin + Send,
    Cli::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Cli::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Cli::Response: hyper_util::client::legacy::connect::Connection,
{
    let mut reqb = hyper::Request::builder().uri(path).method(http_method);
    for (k, v) in headers {
        reqb = reqb.header(k, v);
    }
    reqb = reqb.header("Content-Type", "application/json");

    let body = if let Some(rq) = rq {
        Full::new(Bytes::from(
            serde_json::to_string(&rq).context(format!("{:?}", rq))?,
        ))
    } else {
        Full::new(Bytes::from("".to_string()))
    };

    let http_request = reqb.body(body)?;

    debug!("do_request: Launching HTTP request: {:?}", http_request);

    let http_response = cl.request(http_request).await?;
    let status = http_response.status();

    debug!(
        "do_request: HTTP response with status {} received: {:?}",
        status, http_response
    );

    let headers = http_response.headers().clone();

    let response_body = http_response.into_body().collect().await?.to_bytes();

    if !status.is_success() {
        Err(ApiError::HTTPResponseError(status, body_to_str(response_body)).into())
    } else {
        // Evaluate body_to_str lazily
        if !response_body.is_empty() {
            serde_json::from_reader(response_body.as_ref())
                .map_err(|e| anyhow::Error::from(e).context(body_to_str(response_body)))
                .map(|r| (r, headers))
        } else {
            Ok((Default::default(), headers))
        }
    }
}

/// The Content-Length header is set automatically.
pub async fn do_upload_multipart<
    Req: Serialize + std::fmt::Debug,
    Resp: DeserializeOwned + Clone,
    Cli,
>(
    cl: &TlsClient<Cli, Full<Bytes>>,
    path: &str,
    headers: &[(hyper::header::HeaderName, String)],
    http_method: &str,
    req: Option<Req>,
    data: hyper::body::Bytes,
) -> Result<Resp>
where
    Cli: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Cli::Future: Unpin + Send,
    Cli::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Cli::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Cli::Response: hyper_util::client::legacy::connect::Connection,
{
    let mut reqb = hyper::Request::builder().uri(path).method(http_method);
    for (k, v) in headers {
        reqb = reqb.header(k, v);
    }

    let data = multipart::format_multipart(&req, data)?;
    reqb = reqb.header("Content-Length", data.as_ref().len());
    reqb = reqb.header(
        "Content-Type",
        format!("multipart/related; boundary={}", multipart::MIME_BOUNDARY),
    );

    let body = Full::new(hyper::body::Bytes::from(data.as_ref().to_vec()));
    let http_request = reqb.body(body)?;
    debug!(
        "do_upload_multipart: Launching HTTP request: {:?}",
        http_request
    );
    let http_response = cl.request(http_request).await?;
    let status = http_response.status();
    debug!(
        "do_upload_multipart: HTTP response with status {} received: {:?}",
        status, http_response
    );
    let response_body = http_response.into_body().collect().await?.to_bytes();

    if !status.is_success() {
        Err(ApiError::HTTPResponseError(status, body_to_str(response_body)).into())
    } else {
        serde_json::from_reader(response_body.as_ref())
            .map_err(|e| anyhow::Error::from(e).context(body_to_str(response_body)))
    }
}

/// An ongoing download.
///
/// Note that this does not necessarily result in a download. It is returned by all API methods
/// that are capable of downloading data. Whether a download takes place is determined by the
/// `Content-Type` sent by the server; frequently, the parameters sent in the request determine
/// whether the server starts a download (`Content-Type: whatever`) or sends a response
/// (`Content-Type: application/json`).
pub struct Download<'a, Request, Response, Client> {
    cl: &'a TlsClient<Client, Full<Bytes>>,
    http_method: String,
    uri: hyper::Uri,
    rq: Option<&'a Request>,
    headers: Vec<(hyper::header::HeaderName, String)>,

    _marker: std::marker::PhantomData<Response>,
}

impl<
        'a,
        Request: Serialize + std::fmt::Debug,
        Response: DeserializeOwned + std::fmt::Debug,
        Client,
    > Download<'a, Request, Response, Client>
where
    Client: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Client::Future: Unpin + Send,
    Client::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Client::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Client::Response: hyper_util::client::legacy::connect::Connection,
{
    /// Trivial adapter for `download()`: Store downloaded data into a `Vec<u8>`.
    pub async fn do_it_to_buf(&mut self, buf: &mut Vec<u8>) -> Result<DownloadResult<Response>> {
        self.do_it(Some(buf)).await
    }

    /// Run the actual download, streaming the response into the supplied `dst`. If the server
    /// responded with a `Response` object, no download is started; the response is wrapped in the
    /// `DownloadResult<Response>` object.
    ///
    /// Whether a download takes place or you receive a structured `Response` (i.e. a JSON object)
    /// depends on the `Content-Type` sent by the server. It is an error to attempt a download
    /// without specifying `dst`. Often, whether a download takes place is influenced by the
    /// request parameters. For example, `alt = media` is frequently used in Google APIs to
    /// indicate that a download is expected.
    pub async fn do_it(
        &mut self,
        dst: Option<&mut (dyn AsyncWriteUnpin)>,
    ) -> Result<DownloadResult<Response>> {
        use std::str::FromStr;

        let mut http_response;
        let mut n_redirects = 0;
        let mut uri = self.uri.clone();

        // Follow redirects.
        loop {
            let mut reqb = hyper::Request::builder()
                .uri(&uri)
                .method(self.http_method.as_str());
            for (k, v) in self.headers.iter() {
                reqb = reqb.header(k, v);
            }

            let body = if let Some(rq) = self.rq {
                Full::new(Bytes::from(
                    serde_json::to_string(&rq).context(format!("{:?}", rq))?,
                ))
            } else {
                Full::new(Bytes::from("".to_string()))
            };

            let http_request = reqb.body(body)?;
            debug!(
                "Download::do_it: Redirect {}, Launching HTTP request: {:?}",
                n_redirects, http_request
            );

            http_response = Some(self.cl.request(http_request).await?);
            let status = http_response.as_ref().unwrap().status();
            debug!(
                "Download::do_it: Redirect {}, HTTP response with status {} received: {:?}",
                n_redirects, status, http_response
            );

            // Server returns data - either download or structured response (JSON).
            if status.is_success() {
                let headers = http_response.as_ref().unwrap().headers();

                // Check if an object was returned.
                if let Some(ct) = headers.get(hyper::header::CONTENT_TYPE) {
                    if ct.to_str()?.contains("application/json") {
                        let response_body = http_response
                            .unwrap()
                            .into_body()
                            .collect()
                            .await?
                            .to_bytes();
                        return serde_json::from_reader(response_body.as_ref())
                            .map_err(|e| anyhow::Error::from(e).context(body_to_str(response_body)))
                            .map(DownloadResult::Response);
                    }
                }

                if let Some(dst) = dst {
                    use tokio::io::AsyncWriteExt;
                    let mut response_body = http_response.unwrap().into_body().into_data_stream();
                    while let Some(chunk) = tokio_stream::StreamExt::next(&mut response_body).await
                    {
                        let chunk = chunk?;
                        // Chunks often contain just a few kilobytes.
                        // info!("received chunk with size {}", chunk.as_ref().len());
                        dst.write_all(chunk.as_ref()).await?;
                    }
                    return Ok(DownloadResult::Downloaded);
                } else {
                    return Err(ApiError::DataAvailableError(format!(
                        "No `dst` was supplied to download data to. Content-Type: {:?}",
                        headers.get(hyper::header::CONTENT_TYPE)
                    ))
                    .into());
                }

            // Server redirects us.
            } else if status.is_redirection() {
                n_redirects += 1;
                let new_location = http_response
                    .as_ref()
                    .unwrap()
                    .headers()
                    .get(hyper::header::LOCATION);
                if new_location.is_none() {
                    return Err(ApiError::RedirectError(
                        "Redirect doesn't contain a Location: header".to_string(),
                    )
                    .into());
                }
                uri = hyper::Uri::from_str(new_location.unwrap().to_str()?)?;
                continue;
            } else if !status.is_success() {
                return Err(ApiError::HTTPResponseError(
                    status,
                    body_to_str(
                        http_response
                            .unwrap()
                            .into_body()
                            .collect()
                            .await?
                            .to_bytes(),
                    ),
                )
                .into());
            }

            // Too many redirects.
            if n_redirects > 5 {
                return Err(ApiError::HTTPTooManyRedirectsError.into());
            }
        }
    }
}

pub async fn do_download<
    'a,
    Req: Serialize + std::fmt::Debug,
    Resp: DeserializeOwned + std::fmt::Debug,
    Cli,
>(
    cl: &'a TlsClient<Cli, Full<Bytes>>,
    path: &str,
    headers: Vec<(hyper::header::HeaderName, String)>,
    http_method: String,
    rq: Option<&'a Req>,
) -> Result<Download<'a, Req, Resp, Cli>>
where
    Cli: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Cli::Future: Unpin + Send,
    Cli::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Cli::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Cli::Response: hyper_util::client::legacy::connect::Connection,
{
    use std::str::FromStr;
    Ok(Download {
        cl,
        http_method,
        uri: hyper::Uri::from_str(path)?,
        rq,
        headers,
        _marker: Default::default(),
    })
}

/// A resumable upload in progress, useful for sending large objects.
pub struct ResumableUpload<'client, Response: DeserializeOwned, Client> {
    dest: hyper::Uri,
    cl: &'client TlsClient<Client, Full<Bytes>>,
    max_chunksize: usize,
    _resp: std::marker::PhantomData<Response>,
}

fn format_content_range(from: usize, to: usize, total: usize) -> String {
    format!("bytes {}-{}/{}", from, to, total)
}

fn parse_response_range(rng: &str) -> Option<(usize, usize)> {
    if let Some(main) = rng.strip_prefix("bytes=") {
        let mut parts = main.split("-");
        let (first, second) = (parts.next(), parts.next());
        if first.is_none() || second.is_none() {
            return None;
        }
        Some((
            first.unwrap().parse::<usize>().unwrap_or(0),
            second.unwrap().parse::<usize>().unwrap_or(0),
        ))
    } else {
        None
    }
}

impl<'client, Response: DeserializeOwned, Client> ResumableUpload<'client, Response, Client>
where
    Client: Send + Sync + Clone + tower_service::Service<hyper::Uri> + 'static,
    Client::Future: Unpin + Send,
    Client::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    Client::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    Client::Response: hyper_util::client::legacy::connect::Connection,
{
    pub fn new(
        to: hyper::Uri,
        cl: &'client TlsClient<Client, Full<Bytes>>,
        max_chunksize: usize,
    ) -> ResumableUpload<'client, Response, Client> {
        ResumableUpload {
            dest: to,
            cl,
            max_chunksize,
            _resp: Default::default(),
        }
    }
    pub fn set_max_chunksize(&mut self, size: usize) -> Result<&mut Self> {
        if size % (1024 * 256) != 0 {
            Err(ApiError::InputDataError(
                "ResumableUpload: max_chunksize must be multiple of 256 KiB.".into(),
            )
            .into())
        } else {
            self.max_chunksize = size;
            Ok(self)
        }
    }

    /// Upload data from a reader; use only if the reader cannot be seeked. Memory usage is higher,
    /// because data needs to be cached if the server hasn't accepted all data.
    pub async fn upload<R: tokio::io::AsyncRead + std::marker::Unpin>(
        &self,
        mut f: R,
        size: usize,
    ) -> Result<Response> {
        use tokio::io::AsyncReadExt;

        // Cursor to current position in stream.
        let mut current = 0;
        // Buffer portion that we couldn't send previously.
        let mut previously_unsent = None;
        loop {
            let chunksize = if (size - current) > self.max_chunksize {
                self.max_chunksize
            } else {
                size - current
            };

            let mut buf: Vec<u8>;
            let read_from_stream;
            if let Some(buf2) = previously_unsent.take() {
                buf = buf2;
                read_from_stream = buf.len();
            } else {
                buf = vec![0_u8; chunksize];
                // Move buffer into body.
                read_from_stream = f.read_exact(&mut buf).await?;
                buf.resize(read_from_stream, 0);
            }

            let reqb = hyper::Request::builder()
                .uri(self.dest.clone())
                .method(hyper::Method::PUT)
                .header(hyper::header::CONTENT_LENGTH, read_from_stream)
                .header(
                    hyper::header::CONTENT_RANGE,
                    format_content_range(current, current + read_from_stream - 1, size),
                )
                .header(hyper::header::CONTENT_TYPE, "application/octet-stream");
            let request = reqb.body(Full::new(Bytes::from(buf[..].to_vec())))?;
            debug!("upload_file: Launching HTTP request: {:?}", request);

            let response = self.cl.request(request).await?;
            debug!("upload_file: Received response: {:?}", response);

            let status = response.status();
            // 308 means: continue upload.
            if !status.is_success() && status.as_u16() != 308 {
                debug!("upload_file: Encountered error: {}", status);
                return Err(ApiError::HTTPResponseError(status, status.to_string())).context(
                    body_to_str(response.into_body().collect().await?.to_bytes()),
                );
            }

            let sent;
            if let Some(rng) = response.headers().get(hyper::header::RANGE) {
                if let Some((_, to)) = parse_response_range(rng.to_str()?) {
                    sent = to + 1 - current;
                    if sent < read_from_stream {
                        previously_unsent = Some(buf.split_off(sent));
                    }
                    current = to + 1;
                } else {
                    sent = read_from_stream;
                    current += read_from_stream;
                }
            } else {
                sent = read_from_stream;
                current += read_from_stream;
            }

            debug!(
                "upload_file: Sent {} bytes (successful: {}) of total {} to {}",
                chunksize, sent, size, self.dest
            );

            if current >= size {
                let headers = response.headers().clone();
                let response_body = response.into_body().collect().await?.to_bytes();

                if !status.is_success() {
                    return Err(Error::from(ApiError::HTTPResponseError(
                        status,
                        body_to_str(response_body),
                    ))
                    .context(format!("{:?}", headers)));
                } else {
                    return serde_json::from_reader(response_body.as_ref()).map_err(|e| {
                        anyhow::Error::from(e)
                            .context(body_to_str(response_body))
                            .context(format!("{:?}", headers))
                    });
                }
            }
        }
    }
    /// Upload content from a file. This is most efficient if you have an actual file, as seek can
    /// be used in case the server didn't accept all data.
    pub async fn upload_file(&self, mut f: tokio::fs::File) -> Result<Response> {
        use tokio::io::AsyncReadExt;

        let len = f.metadata().await?.len() as usize;
        let mut current = 0;
        loop {
            let chunksize = if (len - current) > self.max_chunksize {
                self.max_chunksize
            } else {
                len - current
            };

            f.seek(std::io::SeekFrom::Start(current as u64)).await?;

            let mut buf = vec![0_u8; chunksize];
            // Move buffer into body.
            let read_from_stream = f.read_exact(&mut buf).await?;
            buf.resize(read_from_stream, 0);

            let reqb = hyper::Request::builder()
                .uri(self.dest.clone())
                .method(hyper::Method::PUT)
                .header(hyper::header::CONTENT_LENGTH, read_from_stream)
                .header(
                    hyper::header::CONTENT_RANGE,
                    format_content_range(current, current + read_from_stream - 1, len),
                )
                .header(hyper::header::CONTENT_TYPE, "application/octet-stream");
            let request = reqb.body(Full::new(Bytes::from(buf)))?;
            debug!("upload_file: Launching HTTP request: {:?}", request);

            let response = self.cl.request(request).await?;
            debug!("upload_file: Received response: {:?}", response);

            let status = response.status();
            // 308 means: continue upload.
            if !status.is_success() && status.as_u16() != 308 {
                debug!("upload_file: Encountered error: {}", status);
                return Err(ApiError::HTTPResponseError(status, status.to_string())).context(
                    body_to_str(response.into_body().collect().await?.to_bytes()),
                );
            }

            let sent;
            if let Some(rng) = response.headers().get(hyper::header::RANGE) {
                if let Some((_, to)) = parse_response_range(rng.to_str()?) {
                    sent = to + 1 - current;
                    current = to + 1;
                } else {
                    sent = read_from_stream;
                    current += read_from_stream;
                }
            } else {
                // This can also happen if response code is 200.
                sent = read_from_stream;
                current += read_from_stream;
            }

            debug!(
                "upload_file: Sent {} bytes (successful: {}) of total {} to {}",
                chunksize, sent, len, self.dest
            );

            if current >= len {
                let headers = response.headers().clone();
                let response_body = response.into_body().collect().await?.to_bytes();

                if !status.is_success() {
                    return Err(Error::from(ApiError::HTTPResponseError(
                        status,
                        body_to_str(response_body),
                    ))
                    .context(format!("{:?}", headers)));
                } else {
                    return serde_json::from_reader(response_body.as_ref()).map_err(|e| {
                        anyhow::Error::from(e)
                            .context(body_to_str(response_body))
                            .context(format!("{:?}", headers))
                    });
                }
            }
        }
    }
}

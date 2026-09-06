use crate::{http, state::AppState};
use anyhow::{Context, anyhow, bail};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderName, HeaderValue, Request},
};
use http_body_util::BodyExt;
use rtn_mq::{
    MessagingEndpoint, Nack, PublishOptions, Publisher, RecipientOutcome, SubscriptionOptions,
};
use std::{collections::HashMap, io, net::SocketAddr, str::FromStr, time::Duration};
use taskboard_wire::{
    BODY_CHUNK_BYTES, Envelope, FRAME_FORMAT, Frame, Header, REQUEST_TOPIC, RESPONSE_TOPIC,
    RequestId,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;

const MAX_IN_FLIGHT: usize = 128;

struct IncomingRequest {
    next_sequence: u64,
    body: Option<mpsc::Sender<Result<axum::body::Bytes, io::Error>>>,
}

/// Consume authenticated tunnel frames and dispatch them through Taskboard's existing Axum API.
pub async fn run(endpoint: MessagingEndpoint, state: AppState) -> anyhow::Result<()> {
    let mut requests = endpoint
        .subscribe(REQUEST_TOPIC, SubscriptionOptions::acknowledged())
        .await
        .context("subscribe to Taskboard tunnel requests")?;
    let responses = endpoint
        .publisher(RESPONSE_TOPIC)
        .context("open Taskboard tunnel response publisher")?;
    let mut incoming = HashMap::<RequestId, IncomingRequest>::new();

    while let Some(delivery) = requests.recv().await.context("receive tunnel request")? {
        if delivery.format != FRAME_FORMAT {
            delivery.nack(Nack::Permanent).await?;
            continue;
        }
        let envelope = match Envelope::decode(delivery.payload()) {
            Ok(envelope) => envelope,
            Err(error) => {
                tracing::warn!(%error, "rejected malformed tunnel frame");
                delivery.nack(Nack::Permanent).await?;
                continue;
            }
        };
        match accept_frame(
            envelope,
            &mut incoming,
            &endpoint,
            &responses,
            state.clone(),
        )
        .await
        {
            Ok(()) => delivery.ack().await?,
            Err(error) => {
                tracing::warn!(%error, "rejected tunnel request frame");
                delivery.nack(Nack::Permanent).await?;
            }
        }
    }
    bail!("Taskboard tunnel request subscription closed")
}

async fn accept_frame(
    envelope: Envelope,
    incoming: &mut HashMap<RequestId, IncomingRequest>,
    endpoint: &MessagingEndpoint,
    responses: &Publisher,
    state: AppState,
) -> anyhow::Result<()> {
    match envelope.frame {
        Frame::RequestStart {
            request_id,
            method,
            path_and_query,
            headers,
            client_ip,
        } => {
            if incoming.contains_key(&request_id) || incoming.len() >= MAX_IN_FLIGHT {
                bail!("duplicate request or tunnel concurrency limit reached");
            }
            let (body_tx, body_rx) = mpsc::channel(4);
            let mut builder = Request::builder()
                .method(method.as_str())
                .uri(path_and_query);
            for header in headers {
                let name = HeaderName::from_str(&header.name).context("invalid request header")?;
                if is_hop_by_hop(&name) {
                    continue;
                }
                let value =
                    HeaderValue::from_bytes(&header.value).context("invalid request header")?;
                builder = builder.header(name, value);
            }
            let mut request = builder
                .body(Body::from_stream(ReceiverStream::new(body_rx)))
                .context("build tunneled request")?;
            let ip = client_ip
                .parse()
                .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::new(ip, 0)));
            incoming.insert(
                request_id,
                IncomingRequest {
                    next_sequence: 0,
                    body: Some(body_tx),
                },
            );
            let publisher = responses.clone();
            let endpoint = endpoint.clone();
            tokio::spawn(async move {
                if let Err(error) =
                    serve_request(request_id, request, state, endpoint, publisher).await
                {
                    tracing::warn!(%error, "tunneled request failed");
                }
            });
        }
        Frame::RequestChunk {
            request_id,
            sequence,
            bytes,
        } => {
            let request = incoming.get_mut(&request_id).context("unknown request")?;
            if sequence != request.next_sequence {
                bail!("request body frame arrived out of order");
            }
            request.next_sequence += 1;
            if let Some(sender) = request.body.as_ref()
                && sender.send(Ok(bytes.into())).await.is_err()
            {
                request.body = None;
            }
        }
        Frame::RequestEnd { request_id, chunks } => {
            let request = incoming.remove(&request_id).context("unknown request")?;
            if chunks != request.next_sequence {
                bail!("request body ended with a missing frame");
            }
        }
        Frame::RequestCancel { request_id } => {
            incoming.remove(&request_id);
        }
        Frame::ResponseStart { .. } | Frame::ResponseChunk { .. } | Frame::ResponseEnd { .. } => {
            bail!("response frame received on request topic")
        }
    }
    Ok(())
}

async fn serve_request(
    request_id: RequestId,
    request: Request<Body>,
    state: AppState,
    endpoint: MessagingEndpoint,
    publisher: Publisher,
) -> anyhow::Result<()> {
    let response = match http::router(state).oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, mut body) = response.into_parts();
    let headers = encode_headers(&parts.headers)?;
    publish_frame(
        &endpoint,
        &publisher,
        Envelope::new(Frame::ResponseStart {
            request_id,
            status: parts.status.as_u16(),
            headers,
        }),
    )
    .await?;

    let mut sequence = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.context("read Taskboard response body")?;
        if let Ok(data) = frame.into_data() {
            for chunk in data.chunks(BODY_CHUNK_BYTES) {
                if chunk.is_empty() {
                    continue;
                }
                publish_frame(
                    &endpoint,
                    &publisher,
                    Envelope::new(Frame::ResponseChunk {
                        request_id,
                        sequence,
                        bytes: chunk.to_vec(),
                    }),
                )
                .await?;
                sequence += 1;
            }
        }
    }
    publish_frame(
        &endpoint,
        &publisher,
        Envelope::new(Frame::ResponseEnd {
            request_id,
            chunks: sequence,
        }),
    )
    .await
}

pub async fn publish_frame(
    endpoint: &MessagingEndpoint,
    publisher: &Publisher,
    envelope: Envelope,
) -> anyhow::Result<()> {
    let bytes = envelope.encode().context("encode tunnel frame")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let payload = endpoint
            .buffers()
            .copy_from_slice(&bytes)
            .context("reserve rtn-mq payload")?;
        let mut receipt = publisher
            .publish(
                payload,
                PublishOptions {
                    lifetime: Duration::from_secs(120),
                    format: FRAME_FORMAT.into(),
                    ..Default::default()
                },
            )
            .await
            .context("publish tunnel frame")?;
        let outcomes = receipt
            .wait_for_processing(Duration::from_secs(120))
            .await
            .context("wait for tunnel frame acknowledgement")?;
        if outcomes.len() == 1 && matches!(outcomes[0].1, RecipientOutcome::Processed) {
            return Ok(());
        }
        if outcomes.is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        return Err(anyhow!(
            "tunnel peer did not acknowledge frame: {outcomes:?}"
        ));
    }
}

fn encode_headers(headers: &axum::http::HeaderMap) -> anyhow::Result<Vec<Header>> {
    headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| {
            Ok(Header {
                name: name.as_str().to_owned(),
                value: value.as_bytes().to_vec(),
            })
        })
        .collect()
}

pub fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

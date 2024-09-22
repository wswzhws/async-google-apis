mod calendar_v3_types;
use async_google_apis_common as common;
use calendar_v3_types as gcal;
use common::hyper_util::client::legacy::Client;
use common::hyper_util::rt::TokioExecutor;
use hyper_http_proxy::{Intercept, Proxy, ProxyConnector};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let connector = common::hyper_util::client::legacy::connect::HttpConnector::new();
    let proxy = Proxy::new(Intercept::All, "http://127.0.0.1:7890".parse().unwrap());
    let connector = ProxyConnector::from_proxy(connector, proxy)?;

    let client = Client::builder(TokioExecutor::new()).build::<_, String>(connector.clone());

    let secret = common::yup_oauth2::read_application_secret("credentials.json").await?;
    let auth = common::yup_oauth2::InstalledFlowAuthenticator::with_client(
        secret,
        common::yup_oauth2::InstalledFlowReturnMethod::Interactive,
        client.clone(),
    )
    .persist_tokens_to_disk("token.json")
    .build()
    .await?;

    let scopes = vec![
        gcal::CalendarScopes::CalendarReadonly.as_ref(),
        gcal::CalendarScopes::CalendarEventsReadonly.as_ref(),
        gcal::CalendarScopes::Calendar.as_ref(),
        gcal::CalendarScopes::CalendarEvents.as_ref(),
    ];

    let mut cl = gcal::CalendarListService::new(client, Arc::new(auth));
    cl.set_scopes(scopes);
    for cal in gcal_calendars(&cl).await.unwrap().items.unwrap() {
        println!("{:?}", cal);
    }
    Ok(())
}

async fn gcal_calendars<C>(cl: &gcal::CalendarListService<C>) -> anyhow::Result<gcal::CalendarList>
where
    C: Send + Sync + Clone + common::Service<hyper::Uri> + 'static,
    C::Future: Unpin + Send,
    C::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    C::Response: hyper::rt::Read + hyper::rt::Write + Unpin + Send,
    C::Response: common::hyper_util::client::legacy::connect::Connection,
{
    let params = gcal::CalendarListListParams {
        show_deleted: Some(true),
        show_hidden: Some(true),
        ..gcal::CalendarListListParams::default()
    };
    cl.list(&params).await
}

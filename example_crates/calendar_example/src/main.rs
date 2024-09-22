//! This example lists current google calendars

mod calendar_v3_types;
use async_google_apis_common::{self as common, hyper_util::rt::TokioExecutor};
use calendar_v3_types as gcal;
use std::sync::Arc;

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

#[tokio::main]
async fn main() {
    env_logger::init();

    let conn = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .unwrap()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let https = common::hyper_util::client::legacy::Client::builder(TokioExecutor::new())
        .build::<_, String>(conn);

    // Put your client secret in the working directory!
    let sec = common::yup_oauth2::read_application_secret("client_secret.json")
        .await
        .expect("client secret couldn't be read.");
    let auth = common::yup_oauth2::InstalledFlowAuthenticator::builder(
        sec,
        common::yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .persist_tokens_to_disk("tokencache.json")
    .build()
    .await
    .expect("InstalledFlowAuthenticator failed to build");

    let scopes = vec![
        gcal::CalendarScopes::CalendarReadonly,
        gcal::CalendarScopes::CalendarEventsReadonly,
        gcal::CalendarScopes::Calendar,
        gcal::CalendarScopes::CalendarEvents,
    ];

    let mut cl = gcal::CalendarListService::new(https, Arc::new(auth));
    cl.set_scopes(scopes.clone());

    for cal in gcal_calendars(&cl).await.unwrap().items.unwrap() {
        println!("{:?}", cal);
    }
}

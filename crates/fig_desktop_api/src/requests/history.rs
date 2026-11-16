use fig_proto::fig::history_query_request::param::Type;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    HistoryQueryRequest,
    HistoryQueryResponse,
};
use fig_settings::history::History;
use fig_settings::history::rusqlite::params_from_iter;
use fig_settings::history::rusqlite::types::Value;

use super::RequestResult;

pub(crate) async fn query(request: HistoryQueryRequest) -> RequestResult {
    use std::sync::{
        Arc,
        LazyLock,
    };

    use tokio::sync::Semaphore;
    static ADMISSION: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(16)));
    static RUNNING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));
    let admission = ADMISSION
        .clone()
        .try_acquire_owned()
        .map_err(|_error| "Too many pending history queries".to_owned())?;
    let running = RUNNING.clone().acquire_owned().await.map_err(|err| err.to_string())?;
    let mut params: Vec<Value> = Vec::with_capacity(request.params.len());
    for (i, param) in request.params.iter().enumerate() {
        let param = match &param.r#type {
            Some(Type::Null(())) => Value::Null,
            Some(Type::Integer(i)) => Value::Integer(*i),
            Some(Type::Float(f)) => Value::Real(*f),
            Some(Type::String(s)) => Value::Text(s.clone()),
            Some(Type::Blob(b)) => Value::Blob(b.clone()),
            None => return Err(format!("History query parameter {i} is missing type").into()),
        };
        params.push(param);
    }

    let json_array = tokio::task::spawn_blocking(move || {
        let (_admission, _running) = (admission, running);
        let results = History::new()
            .query(&request.query, params_from_iter(params))
            .map_err(|err| format!("Failed querying history: {err}"))?;
        serde_json::to_string(&results).map_err(|err| format!("Failed serializing history: {err}"))
    })
    .await
    .map_err(|err| format!("History worker failed: {err}"))??;
    Ok(ServerOriginatedSubMessage::HistoryQueryResponse(HistoryQueryResponse { json_array }).into())
}

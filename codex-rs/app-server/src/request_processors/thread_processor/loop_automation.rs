//! Fork-owned: `loop/create`, `loop/list`, `loop/delete` app-server requests backed by
//! the native automation extension's durable loop scheduler. Child module of
//! `thread_processor` so it can reuse `load_thread` without editing upstream code.

use super::*;
use codex_app_server_protocol::LoopAutomation;
use codex_app_server_protocol::LoopCreateParams;
use codex_app_server_protocol::LoopCreateResponse;
use codex_app_server_protocol::LoopDeleteParams;
use codex_app_server_protocol::LoopDeleteResponse;
use codex_app_server_protocol::LoopListParams;
use codex_app_server_protocol::LoopListResponse;
use codex_automation::AutomationHandle;
use codex_automation::LoopInfo;

impl ThreadRequestProcessor {
    pub(crate) async fn loop_create(
        &self,
        params: LoopCreateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.loop_create_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn loop_list(
        &self,
        params: LoopListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.loop_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn loop_delete(
        &self,
        params: LoopDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.loop_delete_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    async fn loop_create_inner(
        &self,
        params: LoopCreateParams,
    ) -> Result<LoopCreateResponse, JSONRPCErrorError> {
        let LoopCreateParams {
            thread_id,
            name,
            prompt,
            every_seconds,
        } = params;
        let (_, thread) = self.load_thread(&thread_id).await?;
        let handle = automation_handle(&thread)?;
        let loop_ = handle
            .create_loop(name, prompt, every_seconds)
            .await
            .map_err(invalid_request)?;
        Ok(LoopCreateResponse {
            loop_: api_loop(loop_),
        })
    }

    async fn loop_list_inner(
        &self,
        params: LoopListParams,
    ) -> Result<LoopListResponse, JSONRPCErrorError> {
        let LoopListParams {
            thread_id,
            cursor,
            limit,
        } = params;
        let (_, thread) = self.load_thread(&thread_id).await?;
        let data = automation_handle(&thread)?
            .list_loops()
            .await
            .map_err(internal_error)?
            .into_iter()
            .map(api_loop)
            .collect::<Vec<_>>();
        paginate_loops(data, cursor, limit)
    }

    async fn loop_delete_inner(
        &self,
        params: LoopDeleteParams,
    ) -> Result<LoopDeleteResponse, JSONRPCErrorError> {
        let LoopDeleteParams { thread_id, id } = params;
        let (_, thread) = self.load_thread(&thread_id).await?;
        let loop_ = automation_handle(&thread)?
            .delete_loop(id)
            .await
            .map_err(invalid_request)?;
        Ok(LoopDeleteResponse {
            loop_: api_loop(loop_),
        })
    }
}

fn automation_handle(thread: &CodexThread) -> Result<Arc<AutomationHandle>, JSONRPCErrorError> {
    thread
        .thread_extension_data()
        .get::<AutomationHandle>()
        .ok_or_else(|| internal_error("native automation is unavailable for this thread"))
}

fn api_loop(loop_info: LoopInfo) -> LoopAutomation {
    LoopAutomation {
        id: loop_info.id,
        name: loop_info.name,
        prompt: loop_info.prompt,
        every_seconds: loop_info.every_seconds,
        enabled: loop_info.enabled,
        created_at: loop_info.created_at.div_euclid(1_000),
        next_due_at: loop_info.next_due_at.div_euclid(1_000),
        last_run_at: loop_info
            .last_run_at
            .map(|timestamp| timestamp.div_euclid(1_000)),
    }
}

fn paginate_loops(
    loops: Vec<LoopAutomation>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<LoopListResponse, JSONRPCErrorError> {
    const DEFAULT_PAGE_SIZE: usize = 100;
    const MAX_PAGE_SIZE: usize = 1_000;

    let start = cursor
        .map(|cursor| {
            cursor
                .parse::<usize>()
                .map_err(|_| invalid_request(format!("invalid loop cursor: {cursor}")))
        })
        .transpose()?
        .unwrap_or(0);
    if start > loops.len() {
        return Err(invalid_request(format!(
            "loop cursor {start} exceeds total loops {}",
            loops.len()
        )));
    }
    let page_size = limit
        .map(|limit| limit.max(1) as usize)
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);
    let end = start.saturating_add(page_size).min(loops.len());
    let next_cursor = (end < loops.len()).then(|| end.to_string());
    Ok(LoopListResponse {
        data: loops[start..end].to_vec(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::paginate_loops;
    use codex_app_server_protocol::LoopAutomation;
    use codex_app_server_protocol::LoopListResponse;
    use pretty_assertions::assert_eq;

    fn loop_automation(id: &str) -> LoopAutomation {
        LoopAutomation {
            id: id.to_string(),
            name: format!("loop-{id}"),
            prompt: format!("Run fixture {id}."),
            every_seconds: 60,
            enabled: true,
            created_at: 1,
            next_due_at: 2,
            last_run_at: None,
        }
    }

    #[test]
    fn paginates_with_opaque_offset_cursor() {
        let loops = vec![
            loop_automation("a"),
            loop_automation("b"),
            loop_automation("c"),
        ];
        let first = paginate_loops(loops.clone(), /*cursor*/ None, Some(2))
            .expect("first page should be valid");
        assert_eq!(
            first,
            LoopListResponse {
                data: loops[..2].to_vec(),
                next_cursor: Some("2".to_string()),
            }
        );
        assert_eq!(
            paginate_loops(loops.clone(), first.next_cursor, Some(2))
                .expect("second page should be valid"),
            LoopListResponse {
                data: loops[2..].to_vec(),
                next_cursor: None,
            }
        );
        assert!(paginate_loops(loops, Some("invalid".to_string()), Some(2)).is_err());
    }
}

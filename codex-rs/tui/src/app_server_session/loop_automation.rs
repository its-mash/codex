//! Fork-owned: `loop/create|list|delete` app-server requests issued by the TUI's
//! `/loop` command. Child module of `app_server_session` so it can reuse the
//! session's private request plumbing without editing upstream code.

use super::AppServerSession;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::LoopCreateParams;
use codex_app_server_protocol::LoopCreateResponse;
use codex_app_server_protocol::LoopDeleteParams;
use codex_app_server_protocol::LoopDeleteResponse;
use codex_app_server_protocol::LoopListParams;
use codex_app_server_protocol::LoopListResponse;
use codex_protocol::ThreadId;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;

impl AppServerSession {
    pub(crate) async fn loop_create(
        &mut self,
        thread_id: ThreadId,
        prompt: String,
        every_seconds: u64,
    ) -> Result<LoopCreateResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::LoopCreate {
                request_id,
                params: LoopCreateParams {
                    thread_id: thread_id.to_string(),
                    name: None,
                    prompt,
                    every_seconds,
                },
            })
            .await
            .wrap_err("loop/create failed in TUI")
    }

    pub(crate) async fn loop_list(&mut self, thread_id: ThreadId) -> Result<LoopListResponse> {
        let mut cursor = None;
        let mut data = Vec::new();
        loop {
            let request_id = self.next_request_id();
            let response: LoopListResponse = self
                .client
                .request_typed(ClientRequest::LoopList {
                    request_id,
                    params: LoopListParams {
                        thread_id: thread_id.to_string(),
                        cursor: cursor.clone(),
                        limit: None,
                    },
                })
                .await
                .wrap_err("loop/list failed in TUI")?;
            data.extend(response.data);
            let Some(next_cursor) = response.next_cursor else {
                return Ok(LoopListResponse {
                    data,
                    next_cursor: None,
                });
            };
            if cursor.as_ref() == Some(&next_cursor) {
                return Err(color_eyre::eyre::eyre!(
                    "loop/list returned a repeated pagination cursor"
                ));
            }
            cursor = Some(next_cursor);
        }
    }

    pub(crate) async fn loop_delete(
        &mut self,
        thread_id: ThreadId,
        id: String,
    ) -> Result<LoopDeleteResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::LoopDelete {
                request_id,
                params: LoopDeleteParams {
                    thread_id: thread_id.to_string(),
                    id,
                },
            })
            .await
            .wrap_err("loop/delete failed in TUI")
    }
}

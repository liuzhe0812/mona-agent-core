# Rust测试源码清单 · v0.3.0

## Compaction / Spill 审计修复（2026-09-22）

首次实施后的审计发现单请求不压缩、摘要增大、读取超出 Run 预算和终态清理丢失。已完成修复及针对性回归，详细证据和平台限制见[修复记录](COMPACTION-SPILL-REPAIR.zh-CN.md)。新增真实 HTTP → Provider → Runtime → 归档 → 分页读取 → 压缩 → 完成任务的本地模拟供应商组合测试。以下首次实施记录是历史结果，不能单独作为修复后的交付结论。

## Compaction / Spill 验证（2026-09-22）

本轮定向验证全部通过，没有调用真实付费供应商。Compaction 的 Runtime 集成使用本地确定性模型实现；Server 管理与 Spill 浏览器读取使用进程内 Router；Web 使用 Node 客户端测试。

| 范围 | 结果 |
|---|---|
| `cargo test -p api -p runtime -p compaction -p spill` | 通过；包括 API/Runtime 回归、独立上下文截止时间、短上下文零摘要、受预算摘要、Spill 引用保留、配额/跨 Run/分页/清理/小结果上限 |
| `cargo test -p server` | 11 项通过；因开发中的 `server.exe` 占用，使用独立 `CARGO_TARGET_DIR`，包括能力默认值、重启状态、Spill Bearer 与同 Run 读取 |
| `cargo check --workspace` | 通过 |
| `cargo check -p server --no-default-features` | 通过 |
| `npm run test:web` | 19 项通过；包括 Spill 不透明引用、Bearer、无 Cookie/重定向读取 |
| 正式 Web 能力页 | 真实浏览器通过；Compaction/Spill 当前启用，Skills 当前关闭，桌面列表布局正常 |
| `apps/web/test/renderer.html` | 8 项真实浏览器检查通过；包括 Spill 两页读取、纯文本追加、HTML 不执行和窄屏布局 |

未执行真实供应商摘要联调和原生 Tauri 窗口验证；不能据此宣称这些环境已经生产验收。

## Skills 组件验证（2026-09-22）

新增 12 项 Windows 测试通过，覆盖真实文件 Provider 到 Runtime 的渐进调用、权限与预算、调用策略、路径/格式/大小限制、取消和宿主根目录装配。另有一项 Unix 专用符号链接测试，本轮 Windows 环境未执行。没有调用付费模型。

| 文件 | 测试 | 执行状态 |
|---|---|---|
| `packages/skills/tests/filesystem.rs` | `shallow_catalog_precedence_and_fresh_body_loading` | Windows 已通过 |
| `packages/skills/tests/filesystem.rs` | `invocation_policy_reloads_and_invalid_policies_fail_closed` | Windows 已通过 |
| `packages/skills/tests/filesystem.rs` | `resources_cannot_escape_bundles_or_return_binary_content` | Windows 已通过 |
| `packages/skills/tests/filesystem.rs` | `missing_roots_are_empty_but_io_and_content_limits_are_errors` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `model_discovers_skill_progressively_and_keeps_content_in_tool_history` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `ordinary_runtime_does_not_need_skills_plugin` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `empty_allowed_tools_hides_skill_catalog_and_tool` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `duplicate_skill_names_are_first_provider_wins_even_when_first_is_disabled` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `model_policy_rejects_disabled_skills_but_ignores_user_invocation_flag` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `cancelling_provider_query_returns_without_waiting_for_provider` | Windows 已通过 |
| `packages/skills/tests/runtime.rs` | `oversized_skill_body_stops_run_instead_of_silently_truncating` | Windows 已通过 |
| `apps/server/src/skill_setup.rs` | `skill_roots_follow_project_precedence_and_explicit_isolation` | Windows 已通过 |
| `packages/skills/tests/filesystem.rs` | `symlink_escape_is_rejected_for_instructions_and_resources` | Unix 条件测试，本轮未执行 |

## 原有测试清单

以下原 v0.3 清单记录当时的 143 项测试及历史执行状态；本次可选模型管理新增测试与验证结果见文末。使用 scripts/verify.sh 或 verify.ps1 执行完整验收。

| 文件 | 测试 | 执行状态 |
|---|---|---|
| `packages/http-bridge/tests/http.rs` | `authentication_is_required_even_on_info_and_events` | 未执行 |
| `packages/http-bridge/tests/http.rs` | `http_start_sse_result_and_cursor_reconnect_share_protocol` | 未执行 |
| `packages/http-bridge/tests/http.rs` | `invalid_json_and_unknown_privileged_fields_are_rejected` | 未执行 |
| `packages/http-bridge/tests/http.rs` | `repeated_http_start_is_not_another_model_run` | 未执行 |
| `packages/tauri-bridge/tests/channel.rs` | `channel_waits_for_ack_and_cannot_be_acked_by_another_webview` | 未执行 |
| `packages/tauri-bridge/tests/channel.rs` | `disconnect_detaches_channel_but_does_not_cancel_task` | 未执行 |
| `packages/tauri-bridge/tests/channel.rs` | `unacknowledged_channel_is_evicted_after_timeout` | 未执行 |
| `packages/api/tests/contracts.rs` | `utf8_clipping_never_breaks_a_codepoint` | 未执行 |
| `packages/api/tests/contracts.rs` | `service_lookup_checks_type_and_duplicate_names` | 未执行 |
| `packages/api/tests/contracts.rs` | `message_json_roundtrip_preserves_tool_identity_and_unknown_status` | 未执行 |
| `packages/api/tests/contracts.rs` | `zero_limits_are_rejected` | 未执行 |
| `packages/api/tests/contracts.rs` | `cancellation_child_does_not_cancel_other_task_children` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `legacy_text_stays_a_json_string` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `multimodal_blocks_preserve_order_and_type` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `base64_validation_is_strict_but_does_not_claim_image_decoding` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `resource_reference_is_a_descriptor_not_file_contents` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `content_limits_and_unrecognized_image_media_are_rejected` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `known_tool_error_retains_structured_payload` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `public_tool_result_does_not_copy_media_or_structured_secrets` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `opaque_provider_data_roundtrips_without_reordering_arrays` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `invalid_or_excessive_opaque_data_is_rejected` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `options_inherit_without_merging_opaque_objects` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `invalid_options_and_credential_fields_are_not_accepted` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `artifact_bytes_are_part_of_result_budget` | 未执行 |
| `packages/api/tests/generic_contracts.rs` | `rust_api_version_and_ui_protocol_are_independent` | 未执行 |
| `packages/api/tests/streaming.rs` | `projection_requires_contiguous_sequence_and_correct_run` | 未执行 |
| `packages/api/tests/streaming.rs` | `completed_message_is_authoritative_not_concatenated_twice` | 未执行 |
| `packages/api/tests/streaming.rs` | `ui_unicode_preview_is_bounded_and_flagged` | 未执行 |
| `packages/api/tests/streaming.rs` | `ui_item_retention_is_bounded_and_explicit` | 未执行 |
| `packages/api/tests/streaming.rs` | `streaming_wire_roundtrip_preserves_item_identity` | 未执行 |
| `packages/api/tests/streaming.rs` | `limits_keep_all_in_flight_items_within_snapshot_capacity` | 未执行 |
| `packages/application/tests/application.rs` | `start_is_idempotent_and_rejects_conflicting_body` | 未执行 |
| `packages/application/tests/application.rs` | `both_replay_and_snapshot_end_in_same_outcome` | 未执行 |
| `packages/application/tests/application.rs` | `stale_cursor_gets_snapshot_not_incomplete_deltas` | 未执行 |
| `packages/application/tests/application.rs` | `future_cursor_and_cross_run_lookup_are_rejected` | 未执行 |
| `packages/application/tests/application.rs` | `dropping_subscription_does_not_cancel_run` | 未执行 |
| `packages/application/tests/application.rs` | `subscriptions_and_retained_run_registry_are_bounded` | 未执行 |
| `packages/application/tests/application.rs` | `byte_cap_forces_resync_instead_of_silent_event_loss` | 未执行 |
| `packages/application/tests/application.rs` | `shutdown_closes_admission_without_exposing_host_controls` | 未执行 |
| `packages/application/tests/application.rs` | `public_snapshot_omits_system_prompt_and_request_audits` | 未执行 |
| `packages/application/tests/application.rs` | `cancel_is_a_signal_and_eventually_produces_a_terminal_outcome` | 未执行 |
| `packages/application/tests/application.rs` | `duplicate_input_request_is_applied_once_at_a_safe_boundary` | 未执行 |
| `packages/application/tests/application.rs` | `trusted_model_defaults_are_passed_without_widening_bridge_requests` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `no_sink_keeps_checkpointing_optional` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `checkpoint_order_is_monotonic_and_prior_snapshots_are_immutable` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `tool_body_cannot_run_before_its_intent_is_acknowledged` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `failed_before_model_commit_stops_without_calling_a_model` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `failed_after_model_or_intent_commit_never_dispatches_the_tool` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `failed_result_commit_preserves_known_effect_and_does_not_retry` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `terminal_success_requires_terminal_commit_acknowledgement` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `cancelled_run_still_records_unknown_tool_result_and_final_status` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `checkpoint_timeout_is_bounded_and_does_not_leak_backend_errors` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `oversize_checkpoint_fails_before_model_or_storage_io` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `duplicate_checkpoint_sink_registration_is_not_a_silent_override` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `parallel_tool_commits_serialize_per_run_without_losing_partial_results` | 未执行 |
| `packages/runtime/tests/checkpoints.rs` | `input_receipt_waits_for_checkpoint_ack` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `tool_view_is_recomputed_each_round_without_reregistering` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `per_run_ceiling_cannot_be_expanded_by_a_selector` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `later_selector_cannot_readd_an_earlier_removed_tool` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `hidden_tool_hallucination_never_dispatches` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `empty_run_ceiling_is_not_the_all_tools_default` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `selected_tool_still_requires_final_host_authority` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `tool_selection_obeys_hook_timeout` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `image_input_and_rich_tool_result_survive_the_model_boundary` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `explicit_tool_error_is_not_relabelled_success` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `oversized_rich_output_fails_explicitly_without_sending_broken_media` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `opaque_reply_and_tool_data_are_replayed_but_never_ui_events` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `per_run_model_options_reach_adapter_and_request_audit` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `auxiliary_model_calls_inherit_options_and_share_budget` | 未执行 |
| `packages/runtime/tests/generic_runtime.rs` | `planner_forwards_host_selected_model_options_and_tool_ceiling` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `services_topologically_order_plugins_and_shutdown_reverses_order` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `failed_install_rolls_back_partial_plugin_and_predecessors` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `missing_dependency_and_cycle_are_rejected_before_any_install` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `duplicate_service_declarations_are_rejected_before_install` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `manifest_must_match_actual_published_services` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `installation_timeout_still_calls_shutdown` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `the_model_can_be_supplied_as_a_plugin_service` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `memory_is_a_projection_and_not_a_transcript_rewrite` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `memory_write_tool_is_denied_by_default` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `planner_uses_same_executor_and_shared_task_budget` | 未执行 |
| `packages/runtime/tests/plugins.rs` | `malformed_plan_is_not_executed` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `normal_react_roundtrip_has_paired_results_and_audits` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `complete_json_is_required_before_tool_execution` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `truncated_output_never_executes_even_valid_tool_arguments` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `missing_transport_end_never_executes_a_tool` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `schema_error_is_a_result_not_a_tool_invocation` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `plugin_allow_cannot_bypass_host_side_effect_gate` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `failed_policy_is_fail_closed` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `repeated_call_id_is_not_replayed` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `step_limit_stops_after_settling_tool_results` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `reported_token_breaker_blocks_tool_execution_after_model` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `usage_omission_is_not_reported_as_zero_cost_certainty` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `oversized_unicode_result_is_marked_and_bounded` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `result_transform_cannot_rewrite_execution_status` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `context_transform_must_preserve_valid_call_result_pairing` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `cancel_stops_started_tool_and_preserves_unknown_outcome` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `tool_timeout_does_not_retry` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `tool_panic_is_captured_without_claiming_rollback` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `shutdown_drains_before_revoking_and_old_engine_cannot_start` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `slow_event_reader_does_not_lose_final_result` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `parallel_tools_are_bounded_exclusive_is_a_barrier_results_stay_ordered` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `cancellation_never_starts_queued_tools_after_the_first` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `steering_is_applied_only_after_tool_batch_settlement` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `concurrent_runs_do_not_share_transcripts_or_ids` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `task_call_budget_is_atomic_across_concurrent_users` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `plugin_auxiliary_model_calls_are_counted_against_the_same_budget` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `fragmented_arguments_assemble_before_execution` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `request_audit_limit_rejects_before_network_call` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `orphaned_history_is_rejected` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `external_schema_references_are_rejected_at_registration` | 未执行 |
| `packages/runtime/tests/runtime.rs` | `dropping_execute_future_cancels_child_but_not_shared_task` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `only_api_trait_is_needed_for_stream_cancel_and_result` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `tool_deltas_and_full_terminal_tool_result_share_one_item` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `denied_tool_has_terminal_item_without_running` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `partial_tool_arguments_remain_preview_and_end_skipped` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `slow_core_subscriber_recovers_from_atomic_snapshot` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `hidden_reasoning_is_never_broadcast_as_visible_text` | 未执行 |
| `packages/runtime/tests/streaming.rs` | `structured_tool_details_survive_authoritative_completion` | 未执行 |
| `packages/providers/src/chat.rs` | `usage_only_frame_is_supported` | 未执行 |
| `packages/providers/src/chat.rs` | `length_is_not_success` | 未执行 |
| `packages/providers/src/chat.rs` | `config_cannot_override_messages` | 未执行 |
| `packages/providers/src/chat.rs` | `nonlocal_cleartext_is_rejected` | 未执行 |
| `packages/providers/src/chat.rs` | `reasoning_protocol_data_is_preserved_in_history` | 未执行 |
| `packages/providers/src/chat.rs` | `local_http_stream_and_request_envelope` | 未执行 |
| `packages/providers/src/chat.rs` | `local_http_missing_done_is_an_error` | 未执行 |
| `packages/providers/src/chat.rs` | `local_http_error_is_not_retried` | 未执行 |
| `packages/providers/src/chat.rs` | `image_blocks_are_not_encoded_as_plain_text` | 未执行 |
| `packages/providers/src/chat.rs` | `image_url_needs_opt_in_and_rejects_embedded_credentials` | 未执行 |
| `packages/providers/src/chat.rs` | `unresolved_resource_references_fail_explicitly` | 未执行 |
| `packages/providers/src/chat.rs` | `tool_image_projection_keeps_tool_results_contiguous` | 未执行 |
| `packages/providers/src/chat.rs` | `structured_error_result_is_preserved_in_wire_envelope` | 未执行 |
| `packages/providers/src/chat.rs` | `generation_options_are_explicit_and_model_override_is_allowlisted` | 未执行 |
| `packages/providers/src/chat.rs` | `provider_options_cannot_override_tools_tokens_or_credentials` | 未执行 |
| `packages/providers/src/chat.rs` | `whitelisted_provider_option_reaches_only_its_adapter` | 未执行 |
| `packages/providers/src/chat.rs` | `replay_is_preserved_only_for_the_matching_namespace_and_fields` | 未执行 |
| `packages/providers/src/chat.rs` | `opaque_replay_cannot_rewrite_identity_or_actions` | 未执行 |
| `packages/providers/src/chat.rs` | `complete_snapshot_capture_preserves_order_and_call_association` | 未执行 |
| `packages/providers/src/chat.rs` | `legacy_and_opaque_replay_conflict_is_not_silently_overwritten` | 未执行 |
| `packages/providers/src/chat.rs` | `argument_chunks_treat_empty_identity_as_absent` | 未执行 |
| `packages/providers/src/sse.rs` | `unicode_and_crlf_survive_arbitrary_network_splits` | 未执行 |
| `packages/providers/src/sse.rs` | `one_byte_at_a_time` | 未执行 |
| `packages/providers/src/sse.rs` | `large_unterminated_frame_is_rejected` | 未执行 |
| `packages/providers/src/sse.rs` | `invalid_utf8_is_rejected` | 未执行 |
| `apps/server/tests/shared_application.rs` | `http_created_run_is_visible_through_tauri_without_another_runtime` | 未执行 |

## 可选模型管理新增验证

2026-09-21：下列 18 项通过实际 Cargo 测试；模型请求使用本地 loopback 模拟端点，不代表真实供应商兼容性验收。原 shared_application 回归也已通过。

| 文件 | 测试 | 执行状态 |
|---|---|---|
| `packages/models/src/lib.rs` | `persistence_failures_do_not_publish_and_revisions_prevent_lost_updates` | 已通过 |
| `packages/models/src/lib.rs` | `credentials_are_write_only_and_blank_updates_preserve_them` | 已通过 |
| `packages/models/src/lib.rs` | `defaults_stay_enabled_and_deleting_the_default_clears_it_when_no_fallback_exists` | 已通过 |
| `packages/models/src/lib.rs` | `endpoints_and_model_catalogs_are_validated_before_saving` | 已通过 |
| `packages/models/src/lib.rs` | `bound_models_outlive_configuration_changes_and_release_routes` | 已通过 |
| `packages/models/src/lib.rs` | `plugin_exposes_only_model_and_requires_the_host_runtime_decorator` | 已通过 |
| `packages/models/src/storage.rs` | `encrypted_roundtrip` | 已通过 |
| `packages/models/src/storage.rs` | `file_does_not_contain_plaintext` | 已通过 |
| `packages/models/src/storage.rs` | `wrong_key_and_tampering_fail_as_configuration_errors` | 已通过 |
| `packages/models/src/storage.rs` | `missing_file_returns_none` | 已通过 |
| `packages/models/src/storage.rs` | `save_overwrites_previous_settings` | 已通过 |
| `packages/models/src/storage.rs` | `oversized_settings_are_rejected` | 已通过 |
| `packages/models/src/storage.rs` | `short_key_material_is_rejected` | 已通过 |
| `apps/server/src/model_settings_tests.rs` | `management_routes_require_bearer_authentication` | 已通过 |
| `apps/server/src/model_settings_tests.rs` | `management_rejects_unknown_fields_and_does_not_return_api_keys` | 已通过 |
| `apps/server/src/model_settings_tests.rs` | `management_uses_revision_conflicts_and_controls_default_visibility_and_delete` | 已通过 |
| `apps/server/src/model_settings_tests.rs` | `new_runs_use_the_current_default_model` | 已通过 |
| `apps/server/src/model_settings_tests.rs` | `changing_default_during_a_tool_run_does_not_change_its_next_round` | 已通过 |

## Agent 能力管理验证

2026-09-22：部署策略、用户状态持久化、重启状态表达、Bearer 管理路由和 Web 客户端共 7 项通过。

| 文件 | 测试 | 执行状态 |
|---|---|---|
| `apps/server/src/capabilities.rs` | `user_choice_is_persisted_and_only_becomes_active_after_reopen` | 已通过 |
| `apps/server/src/capabilities.rs` | `deployment_locked_capability_cannot_be_changed` | 已通过 |
| `apps/server/src/capabilities.rs` | `management_route_requires_auth_and_returns_restart_state` | 已通过 |
| `apps/web/capabilities.test.mjs` | 4 项请求隔离、更新、冲突与参数校验 | 已通过 |

## Harness 可靠性优化验证

2026-09-22：下列定向回归覆盖模型重试、上下文恢复、参数诊断、审计模式和执行/UI 解耦。

| 文件 | 测试 | 执行状态 |
|---|---|---|
| `packages/providers/src/chat.rs` | `context_overflow_is_machine_readable_without_exposing_the_body` | 已通过 |
| `packages/providers/src/chat.rs` | `deterministic_http_request_error_is_not_retryable_transport` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `transient_model_failure_retries_within_budget` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `model_retry_wait_is_cancelled_with_the_run` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `authentication_and_partial_output_are_never_retried` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `schema_error_details_are_bounded_and_report_missing_paths` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `metadata_audit_omits_request_bodies_and_reduces_report_size` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `execution_settles_more_tools_than_the_ui_snapshot_retains` | 已通过 |
| `packages/api/tests/streaming.rs` | `execution_limit_is_independent_from_snapshot_retention` | 已通过 |
| `packages/compaction/src/lib.rs` | `known_model_window_triggers_before_the_byte_ceiling` | 已通过 |
| `packages/compaction/src/lib.rs` | `known_model_window_rejects_an_unshrinkable_protected_tail` | 已通过 |
| `packages/compaction/tests/runtime.rs` | `confirmed_context_overflow_forces_one_smaller_retry` | 已通过 |
| `packages/runtime/tests/streaming.rs` | `partial_message_is_retained_when_model_fails_after_text` | 已通过 |
| `packages/runtime/src/events.rs` | `evicted_active_tool_keeps_deltas_details_and_terminal_result` | 已通过 |
| `packages/runtime/tests/runtime.rs` | `adapter_reported_partial_output_prevents_retry` | 已通过 |
| `packages/compaction/tests/runtime.rs` | `summary_calls_also_fit_the_known_model_window` | 已通过 |
| `packages/providers/src/chat.rs` | `provider_error_classification_uses_identifiers_and_strict_messages` | 已通过 |
| `packages/providers/src/chat.rs` | `http_status_precedence_and_conflict_default_are_stable` | 已通过 |
| `packages/providers/src/chat.rs` | `replay_index_uses_the_api_tool_call_limit` | 已通过 |
| `packages/providers/src/chat.rs` | `context_capacity_only_applies_to_the_configured_model` | 已通过 |

审计模式先以同一确定性模型、同一 32 KiB 输入和相同预算做成对测量：两种模式均一次完成；完整审计 JSON 为 33,086 字节，精简审计为 150 字节。

返修后又以 12 轮确定性工具任务重复 3 次：每次均完成 12 次工具调用、13 次模型调用和 63 次检查点提交。完整审计均为 343,571 字节，精简审计均为 1,314 字节；检查点累计序列化量均为 1,718,040 字节，单次快照序列化量峰值 52,398 字节。三次完整/精简耗时分别为 66.0/64.5、65.2/65.8、66.6/65.7 毫秒，仅作本机诊断，不代表真实供应商延迟。该探针检查了提交数量与序列化量，没有逐项比较提交顺序，也没有测量进程峰值内存或单独分离复制开销。结果只确认此样例下审计正文省略生效、检查点序列化量一致，不据此宣称全部长任务成本已解决；完整检查点契约保持不变。

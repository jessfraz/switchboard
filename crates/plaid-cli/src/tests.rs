use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use super::{
    cache::{AccountSnapshotSource, PlaidCacheStore},
    main_entry, render_cli_error, run,
    state::{PlaidEnvironment, PlaidState, StateStore, DEFAULT_PLAID_VERSION},
    Cli,
};

#[test]
fn main_entry_returns_success_for_help() {
    assert_eq!(main_entry(["plaid", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn exchange_public_token_stores_access_token_and_item_id() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "access_token": "access-sandbox-1234",
            "item_id": "item-1234",
            "request_id": "request-1"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-exchange-public-token");
    let config_path = temp_dir.join("config.json");

    let output: ExchangePublicTokenResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--environment",
        "sandbox",
        "--base-url",
        &server.base_url(),
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "--compact",
        "auth",
        "exchange-public-token",
        "--public-token",
        "public-sandbox-abc",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/item/public_token/exchange");
    assert_eq!(request.header("plaid-client-id"), Some("client-id"));
    assert_eq!(request.header("plaid-secret"), Some("secret-value"));
    assert_eq!(request.header("plaid-version"), Some(DEFAULT_PLAID_VERSION));
    assert_eq!(body["public_token"], "public-sandbox-abc");

    let state = StateStore::new(config_path).load().expect("state should load");
    assert_eq!(state.environment, Some(PlaidEnvironment::Sandbox));
    assert_eq!(state.access_token.as_deref(), Some("access-sandbox-1234"));
    assert_eq!(state.item_id.as_deref(), Some("item-1234"));
    assert_eq!(output.access_token, "access-sandbox-1234");
}

#[test]
fn link_token_create_builds_user_products_and_transactions_options() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "link_token": "link-sandbox-1234",
            "expiration": "2026-04-01T00:00:00Z",
            "request_id": "request-2"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-link-token");
    let config_path = temp_dir.join("config.json");

    let output: LinkTokenCreateResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &server.base_url(),
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "--client-name",
        "switchboard",
        "--compact",
        "link",
        "token-create",
        "--client-user-id",
        "user-123",
        "--product",
        "transactions",
        "--product",
        "auth",
        "--country-code",
        "US",
        "--country-code",
        "CA",
        "--days-requested",
        "180",
        "--webhook",
        "https://example.com/plaid-webhook",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/link/token/create");
    assert_eq!(body["client_name"], "switchboard");
    assert_eq!(body["language"], "en");
    assert_eq!(body["user"]["client_user_id"], "user-123");
    assert_eq!(body["products"], json!(["transactions", "auth"]));
    assert_eq!(body["country_codes"], json!(["US", "CA"]));
    assert_eq!(body["transactions"]["days_requested"], 180);
    assert_eq!(body["webhook"], "https://example.com/plaid-webhook");
    assert_eq!(output.link_token, "link-sandbox-1234");
}

#[test]
fn link_token_create_supports_optional_required_additional_products_and_account_filters() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "link_token": "link-sandbox-advanced",
            "expiration": "2026-04-01T00:00:00Z",
            "request_id": "request-link-advanced"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-link-token-advanced");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: LinkTokenCreateResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "link",
        "token-create",
        "--client-user-id",
        "user-123",
        "--product",
        "transactions",
        "--optional-product",
        "auth",
        "--required-if-supported-product",
        "identity",
        "--additional-consented-product",
        "balance_plus",
        "--country-code",
        "US",
        "--update-mode",
        "--link-customization-name",
        "default",
        "--android-package-name",
        "com.example.switchboard",
        "--routing-number",
        "021000021",
        "--redirect-uri",
        "https://jessfraz.github.io/switchboard/plaid-callback/",
        "--depository-subtype",
        "checking",
        "--investment-subtype",
        "brokerage",
        "--days-requested",
        "90",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/link/token/create");
    assert_eq!(body["products"], json!(["transactions"]));
    assert_eq!(body["optional_products"], json!(["auth"]));
    assert_eq!(body["required_if_supported_products"], json!(["identity"]));
    assert_eq!(body["additional_consented_products"], json!(["balance_plus"]));
    assert_eq!(body["access_token"], "stored-access-token");
    assert_eq!(body["link_customization_name"], "default");
    assert_eq!(body["android_package_name"], "com.example.switchboard");
    assert_eq!(
        body["redirect_uri"],
        "https://jessfraz.github.io/switchboard/plaid-callback/"
    );
    assert_eq!(body["institution_data"]["routing_number"], "021000021");
    assert_eq!(
        body["account_filters"]["depository"]["account_subtypes"],
        json!(["checking"])
    );
    assert_eq!(
        body["account_filters"]["investment"]["account_subtypes"],
        json!(["brokerage"])
    );
    assert_eq!(body["transactions"]["days_requested"], 90);
    assert_eq!(output.link_token, "link-sandbox-advanced");
}

#[test]
fn link_token_create_rejects_additional_consented_products_without_update_mode() {
    let error: JsonErrorResponse = run_command_error(&[
        "plaid",
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "link",
        "token-create",
        "--client-user-id",
        "user-123",
        "--product",
        "transactions",
        "--additional-consented-product",
        "balance_plus",
    ]);

    assert_eq!(error.kind, "arguments");
    assert_eq!(error.message, "--additional-consented-product requires --update-mode");
}

#[test]
fn item_get_caches_item_and_remembers_item_id() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "item": {
                "item_id": "item-1234",
                "institution_id": "ins_109508",
                "webhook": "https://example.com/plaid",
                "error": null
            },
            "status": {
                "transactions": {
                    "last_successful_update": "2026-03-30T00:00:00Z"
                }
            },
            "request_id": "request-item"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-accounts-balance");
    let config_path = temp_dir.join("config.json");
    let store = StateStore::new(config_path.clone());
    store
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "item",
        "get",
    ]);

    let request = captured_request(&capture);
    assert_eq!(request.path, "/item/get");
    assert_eq!(output["item"]["item_id"], "item-1234");

    let state = StateStore::new(config_path.clone()).load().expect("state should load");
    assert_eq!(state.item_id.as_deref(), Some("item-1234"));
    let item = cached_item(&cache_db_path(&config_path), "item-1234");
    assert_eq!(item["institution_id"], "ins_109508");
}

#[test]
fn accounts_balance_uses_stored_access_token_filters_and_caches_accounts() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "accounts": [
                {
                    "account_id": "acc-123",
                    "balances": {
                        "available": 100.5,
                        "current": 125.0
                    },
                    "mask": "0000",
                    "name": "Cash"
                }
            ],
            "item": {
                "item_id": "item-1234",
                "institution_id": "ins_109508"
            },
            "request_id": "request-3"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-sync");
    let config_path = temp_dir.join("config.json");
    let store = StateStore::new(config_path.clone());
    store
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: AccountsBalanceResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "accounts",
        "balance",
        "--account-id",
        "acc-123",
        "--min-last-updated-datetime",
        "2026-03-30T00:00:00Z",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/accounts/balance/get");
    assert_eq!(body["access_token"], "stored-access-token");
    assert_eq!(body["options"]["account_ids"], json!(["acc-123"]));
    assert_eq!(body["options"]["min_last_updated_datetime"], "2026-03-30T00:00:00Z");
    assert_eq!(output.accounts[0].account_id, "acc-123");

    let item = cached_item(&cache_db_path(&config_path), "item-1234");
    assert_eq!(item["institution_id"], "ins_109508");
    let account = cached_account(&cache_db_path(&config_path), "acc-123");
    assert_eq!(account["balances"]["available"], 100.5);
    assert_eq!(account["name"], "Cash");

    let cached: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "accounts",
    ]);
    assert_eq!(cached["accounts"][0]["source_endpoint"], "/accounts/balance/get");
    assert_eq!(cached["accounts"][0]["balance_freshness"], "realtime");
}

#[test]
fn institutions_get_by_id_builds_country_codes_and_metadata_options() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "institution": {
                "institution_id": "ins_109508",
                "name": "First Platypus Bank"
            },
            "request_id": "request-institution"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let output: Value = run_command(&[
        "plaid",
        "--base-url",
        &server.base_url(),
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "--compact",
        "institutions",
        "get-by-id",
        "ins_109508",
        "--country-code",
        "US",
        "--include-status",
        "--include-auth-metadata",
        "--include-payment-initiation-metadata",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/institutions/get_by_id");
    assert_eq!(body["institution_id"], "ins_109508");
    assert_eq!(body["country_codes"], json!(["US"]));
    assert_eq!(body["options"]["include_status"], true);
    assert_eq!(body["options"]["include_auth_metadata"], true);
    assert_eq!(body["options"]["include_payment_initiation_metadata"], true);
    assert_eq!(output["institution"]["name"], "First Platypus Bank");
}

#[test]
fn transactions_refresh_posts_access_token_and_reports_item_id() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        json!({
            "request_id": "request-refresh"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-refresh");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            item_id: Some("item-1234".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "refresh",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/transactions/refresh");
    assert_eq!(body["access_token"], "stored-access-token");
    assert_eq!(output["item_id"], "item-1234");
    assert_eq!(output["request_id"], "request-refresh");
}

#[test]
fn transactions_sync_uses_cached_cursor_paginates_and_updates_cache() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn_sequence(
        vec![
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-1",
                            "account_id": "acc-123",
                            "amount": 12.34,
                            "name": "Coffee"
                        }
                    ],
                    "modified": [],
                    "removed": [],
                    "next_cursor": "cursor-page-1",
                    "has_more": true,
                    "request_id": "request-page-1"
                }),
                200,
            ),
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-2",
                            "account_id": "acc-123",
                            "amount": 56.78,
                            "name": "Groceries"
                        }
                    ],
                    "modified": [
                        {
                            "transaction_id": "tx-1",
                            "account_id": "acc-123",
                            "amount": 22.34,
                            "name": "Coffee Shop"
                        }
                    ],
                    "removed": [
                        {
                            "transaction_id": "tx-3",
                            "account_id": "acc-123"
                        }
                    ],
                    "next_cursor": "cursor-final",
                    "has_more": false,
                    "request_id": "request-page-2"
                }),
                200,
            ),
        ],
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-sync");
    let config_path = temp_dir.join("config.json");
    let store = StateStore::new(config_path.clone());
    store
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            item_id: Some("item-1234".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: TransactionsSyncResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "sync",
        "--count",
        "250",
        "--days-requested",
        "180",
        "--account-id",
        "acc-123",
        "--include-original-description",
    ]);

    let requests = captured_requests(&capture);
    assert_eq!(requests.len(), 2);
    let first_body: Value = requests[0].json_body();
    assert_eq!(requests[0].path, "/transactions/sync");
    assert!(first_body.get("cursor").is_none());
    assert_eq!(first_body["count"], 250);
    assert_eq!(first_body["options"]["account_id"], "acc-123");
    assert_eq!(first_body["options"]["days_requested"], 180);
    assert_eq!(first_body["options"]["include_original_description"], true);

    let second_body: Value = requests[1].json_body();
    assert_eq!(second_body["cursor"], "cursor-page-1");
    assert_eq!(output.next_cursor, "cursor-final");
    assert_eq!(output.pages_fetched, 2);

    assert_eq!(
        cached_cursor(&cache_db_path(&config_path), "item-1234", Some("acc-123")).as_deref(),
        Some("cursor-final")
    );
    let updated_transaction = cached_transaction(&cache_db_path(&config_path), "tx-1");
    assert_eq!(updated_transaction["amount"], 22.34);
    let added_transaction = cached_transaction(&cache_db_path(&config_path), "tx-2");
    assert_eq!(added_transaction["name"], "Groceries");
    let removed_transaction = cached_transaction(&cache_db_path(&config_path), "tx-3");
    assert_eq!(removed_transaction["account_id"], "acc-123");
    assert!(cached_transaction_removed(&cache_db_path(&config_path), "tx-3"));

    let second_capture = Arc::new(Mutex::new(Vec::new()));
    let second_server = TestServer::spawn(
        json!({
            "added": [],
            "modified": [],
            "removed": [],
            "next_cursor": "cursor-after-cache",
            "has_more": false,
            "request_id": "request-page-3"
        })
        .to_string(),
        200,
        Some(second_capture.clone()),
    );
    let second_output: TransactionsSyncResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &second_server.base_url(),
        "--compact",
        "transactions",
        "sync",
        "--account-id",
        "acc-123",
    ]);

    let cached_request = captured_request(&second_capture);
    let cached_body: Value = cached_request.json_body();
    assert_eq!(cached_body["cursor"], "cursor-final");
    assert_eq!(second_output.next_cursor, "cursor-after-cache");
    assert_eq!(second_output.pages_fetched, 1);
}

#[test]
fn transactions_sync_bootstraps_item_id_before_caching() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn_sequence(
        vec![
            ResponseSpec::json(
                json!({
                    "item": {
                        "item_id": "item-bootstrap",
                        "institution_id": "ins_109508"
                    },
                    "request_id": "request-item"
                }),
                200,
            ),
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-bootstrap",
                            "account_id": "acc-123",
                            "amount": 19.99,
                            "name": "Bootstrap"
                        }
                    ],
                    "modified": [],
                    "removed": [],
                    "next_cursor": "cursor-bootstrap",
                    "has_more": false,
                    "request_id": "request-sync"
                }),
                200,
            ),
        ],
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-bootstrap-item");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "sync",
    ]);

    let requests = captured_requests(&capture);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/item/get");
    assert_eq!(requests[1].path, "/transactions/sync");
    assert_eq!(output["item_id"], "item-bootstrap");
    assert_eq!(output["next_cursor"], "cursor-bootstrap");

    let state = StateStore::new(config_path.clone()).load().expect("state should load");
    assert_eq!(state.item_id.as_deref(), Some("item-bootstrap"));
    assert_eq!(
        cached_cursor(&cache_db_path(&config_path), "item-bootstrap", None).as_deref(),
        Some("cursor-bootstrap")
    );
    let cached = cached_transaction(&cache_db_path(&config_path), "tx-bootstrap");
    assert_eq!(cached["name"], "Bootstrap");
}

#[test]
fn transactions_sync_restarts_from_initial_cursor_after_pagination_mutation() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn_sequence(
        vec![
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-stale",
                            "account_id": "acc-123",
                            "amount": 1.23,
                            "name": "Stale"
                        }
                    ],
                    "modified": [],
                    "removed": [],
                    "next_cursor": "cursor-page-1",
                    "has_more": true,
                    "request_id": "request-page-1"
                }),
                200,
            ),
            ResponseSpec::json(
                json!({
                    "error_type": "TRANSACTIONS_ERROR",
                    "error_code": "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION",
                    "error_message": "transactions changed during pagination",
                    "request_id": "request-mutation"
                }),
                400,
            ),
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-1",
                            "account_id": "acc-123",
                            "amount": 12.34,
                            "name": "Coffee"
                        }
                    ],
                    "modified": [],
                    "removed": [],
                    "next_cursor": "cursor-page-1b",
                    "has_more": true,
                    "request_id": "request-page-1b"
                }),
                200,
            ),
            ResponseSpec::json(
                json!({
                    "added": [
                        {
                            "transaction_id": "tx-2",
                            "account_id": "acc-123",
                            "amount": 56.78,
                            "name": "Groceries"
                        }
                    ],
                    "modified": [],
                    "removed": [],
                    "next_cursor": "cursor-final",
                    "has_more": false,
                    "request_id": "request-page-2"
                }),
                200,
            ),
        ],
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-restart");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            item_id: Some("item-1234".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "sync",
        "--account-id",
        "acc-123",
    ]);

    let requests = captured_requests(&capture);
    assert_eq!(requests.len(), 4);
    let first_body: Value = requests[0].json_body();
    let second_body: Value = requests[1].json_body();
    let third_body: Value = requests[2].json_body();
    let fourth_body: Value = requests[3].json_body();
    assert!(first_body.get("cursor").is_none());
    assert_eq!(second_body["cursor"], "cursor-page-1");
    assert!(third_body.get("cursor").is_none());
    assert_eq!(fourth_body["cursor"], "cursor-page-1b");
    assert_eq!(output["restart_count"], 1);
    assert_eq!(output["pages_fetched"], 2);
    assert_eq!(output["request_ids"], json!(["request-page-1b", "request-page-2"]));
    assert_eq!(
        cached_cursor(&cache_db_path(&config_path), "item-1234", Some("acc-123")).as_deref(),
        Some("cursor-final")
    );
    assert_eq!(
        cached_transaction(&cache_db_path(&config_path), "tx-1")["name"],
        "Coffee"
    );
    assert_eq!(
        cached_transaction(&cache_db_path(&config_path), "tx-2")["name"],
        "Groceries"
    );
    assert_eq!(
        row_count(
            &cache_db_path(&config_path),
            "plaid_transactions",
            "transaction_id = 'tx-stale'"
        ),
        0
    );
}

#[test]
fn sandbox_public_token_create_rejects_transaction_options_without_transactions_product() {
    let error: JsonErrorResponse = run_command_error(&[
        "plaid",
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "sandbox",
        "public-token-create",
        "--institution-id",
        "ins_109508",
        "--product",
        "auth",
        "--days-requested",
        "180",
    ]);

    assert_eq!(error.kind, "arguments");
    assert_eq!(
        error.message,
        "transaction sandbox options require --product transactions"
    );
}

#[test]
fn cache_items_defaults_to_current_item_and_supports_all() {
    let temp_dir = temp_dir("plaid-cache-items");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            item_id: Some("item-a".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let cache = PlaidCacheStore::open(cache_db_path(&config_path)).expect("cache should open");
    cache
        .cache_item(&json!({
            "item_id": "item-a",
            "institution_id": "ins_1",
            "webhook": "https://example.com/a"
        }))
        .expect("item a should cache");
    cache
        .cache_item(&json!({
            "item_id": "item-b",
            "institution_id": "ins_2",
            "webhook": "https://example.com/b"
        }))
        .expect("item b should cache");

    let current: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "items",
    ]);
    assert_eq!(current["source"], "cache");
    assert_eq!(current["item_id"], "item-a");
    assert_eq!(current["count"], 1);
    assert_eq!(current["items"][0]["item_id"], "item-a");

    let all: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "items",
        "--all",
    ]);
    assert_eq!(all["count"], 2);
    assert_eq!(
        all["items"]
            .as_array()
            .expect("items should be an array")
            .iter()
            .map(|item| item["item_id"].as_str().expect("item_id should be a string"))
            .collect::<Vec<_>>(),
        vec!["item-a", "item-b"]
    );
}

#[test]
fn cache_accounts_reads_cached_accounts_without_credentials() {
    let temp_dir = temp_dir("plaid-cache-accounts");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            item_id: Some("item-a".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let cache = PlaidCacheStore::open(cache_db_path(&config_path)).expect("cache should open");
    cache
        .cache_accounts(
            "item-a",
            &[json!({
                "account_id": "acc-a",
                "name": "Checking",
                "balances": { "available": 11.0 }
            })],
            AccountSnapshotSource::AccountsGet,
        )
        .expect("account a should cache");
    cache
        .cache_accounts(
            "item-b",
            &[json!({
                "account_id": "acc-b",
                "name": "Savings",
                "balances": { "available": 22.0 }
            })],
            AccountSnapshotSource::AccountsBalanceGet,
        )
        .expect("account b should cache");

    let current: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "accounts",
    ]);
    assert_eq!(current["source"], "cache");
    assert_eq!(current["item_id"], "item-a");
    assert_eq!(current["count"], 1);
    assert_eq!(current["accounts"][0]["account_id"], "acc-a");
    assert_eq!(current["accounts"][0]["account"]["name"], "Checking");
    assert_eq!(current["accounts"][0]["source_endpoint"], "/accounts/get");
    assert_eq!(current["accounts"][0]["balance_freshness"], "cached");

    let filtered: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "accounts",
        "--all",
        "--account-id",
        "acc-b",
    ]);
    assert_eq!(filtered["count"], 1);
    assert_eq!(filtered["account_ids"], json!(["acc-b"]));
    assert_eq!(filtered["accounts"][0]["item_id"], "item-b");
    assert_eq!(filtered["accounts"][0]["account"]["balances"]["available"], 22.0);
    assert_eq!(filtered["accounts"][0]["source_endpoint"], "/accounts/balance/get");
    assert_eq!(filtered["accounts"][0]["balance_freshness"], "realtime");
}

#[test]
fn cache_transactions_reads_cached_transactions_and_cursor_without_network() {
    let temp_dir = temp_dir("plaid-cache-transactions");
    let config_path = temp_dir.join("config.json");
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            item_id: Some("item-a".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let cache = PlaidCacheStore::open(cache_db_path(&config_path)).expect("cache should open");
    cache
        .cache_transactions_sync(
            "item-a",
            Some("acc-a"),
            "cursor-a",
            &[
                json!({
                    "transaction_id": "tx-a",
                    "account_id": "acc-a",
                    "amount": 11.25,
                    "name": "Lunch"
                }),
                json!({
                    "transaction_id": "tx-removed",
                    "account_id": "acc-a",
                    "amount": 18.75,
                    "name": "Dinner"
                }),
            ],
            &[],
            &[],
        )
        .expect("initial transactions should cache");
    cache
        .cache_transactions_sync(
            "item-a",
            Some("acc-a"),
            "cursor-a-removed",
            &[],
            &[],
            &[json!({
                "transaction_id": "tx-removed",
                "account_id": "acc-a",
                "pending_transaction_id": "pending-tx-removed"
            })],
        )
        .expect("removed transactions should cache");
    cache
        .cache_transactions_sync(
            "item-b",
            Some("acc-b"),
            "cursor-b",
            &[json!({
                "transaction_id": "tx-b",
                "account_id": "acc-b",
                "amount": 48.50,
                "name": "Books"
            })],
            &[],
            &[],
        )
        .expect("other item transactions should cache");

    let current: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "transactions",
        "--account-id",
        "acc-a",
    ]);
    assert_eq!(current["source"], "cache");
    assert_eq!(current["item_id"], "item-a");
    assert_eq!(current["account_id"], "acc-a");
    assert_eq!(current["cursor"], "cursor-a-removed");
    assert_eq!(current["count"], 1);
    assert_eq!(current["transactions"][0]["transaction_id"], "tx-a");
    assert_eq!(current["transactions"][0]["removed"], false);

    let removed: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "cache",
        "transactions",
        "--include-removed",
        "--transaction-id",
        "tx-removed",
    ]);
    assert_eq!(removed["count"], 1);
    assert_eq!(removed["include_removed"], true);
    assert_eq!(removed["transactions"][0]["transaction_id"], "tx-removed");
    assert_eq!(removed["transactions"][0]["removed"], true);
    assert_eq!(removed["transactions"][0]["account_id"], "acc-a");
    assert_eq!(removed["transactions"][0]["transaction"]["name"], "Dinner");
    assert_eq!(removed["transactions"][0]["transaction"]["amount"], 18.75);
    assert_eq!(
        removed["transactions"][0]["removal"]["pending_transaction_id"],
        "pending-tx-removed"
    );
}

#[test]
fn cache_store_migrates_transaction_tombstone_columns_without_losing_snapshots() {
    let temp_dir = temp_dir("plaid-cache-migration");
    let config_path = temp_dir.join("config.json");
    let cache_path = cache_db_path(&config_path);
    let connection = Connection::open(&cache_path).expect("cache db should open");
    connection
        .execute_batch(
            "CREATE TABLE plaid_transactions (
               transaction_id TEXT PRIMARY KEY,
               item_id TEXT NOT NULL,
               account_id TEXT,
               data_json TEXT NOT NULL,
               is_removed INTEGER NOT NULL DEFAULT 0,
               updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             INSERT INTO plaid_transactions (transaction_id, item_id, account_id, data_json, is_removed)
             VALUES (
               'tx-legacy',
               'item-a',
               'acc-a',
               '{\"transaction_id\":\"tx-legacy\",\"account_id\":\"acc-a\",\"amount\":44.10,\"name\":\"Legacy\"}',
               0
             );",
        )
        .expect("legacy schema should initialize");

    let cache = PlaidCacheStore::open(&cache_path).expect("cache should migrate");
    cache
        .cache_transactions_sync(
            "item-a",
            Some("acc-a"),
            "cursor-migrated",
            &[],
            &[],
            &[json!({
                "transaction_id": "tx-legacy",
                "account_id": "acc-a",
                "pending_transaction_id": "pending-legacy"
            })],
        )
        .expect("removed transaction should cache after migration");

    let migrated = cached_transaction(&cache_path, "tx-legacy");
    assert_eq!(migrated["name"], "Legacy");
    assert_eq!(migrated["amount"], 44.10);
    assert_eq!(migrated["account_id"], "acc-a");
    assert!(cached_transaction_removed(&cache_path, "tx-legacy"));

    let connection = Connection::open(&cache_path).expect("cache db should reopen");
    let removal_json: String = connection
        .query_row(
            "SELECT removed_json FROM plaid_transactions WHERE transaction_id = ?1",
            params!["tx-legacy"],
            |row| row.get(0),
        )
        .expect("removed json should exist");
    let removal: Value = serde_json::from_str(&removal_json).expect("removed json should parse");
    assert_eq!(removal["pending_transaction_id"], "pending-legacy");

    let removed_at: String = connection
        .query_row(
            "SELECT removed_at FROM plaid_transactions WHERE transaction_id = ?1",
            params!["tx-legacy"],
            |row| row.get(0),
        )
        .expect("removed_at should exist");
    assert!(!removed_at.is_empty());
}

#[test]
fn item_remove_bootstraps_item_id_and_purges_local_state_and_cache() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn_sequence(
        vec![
            ResponseSpec::json(
                json!({
                    "item": {
                        "item_id": "item-remove",
                        "institution_id": "ins_109508"
                    },
                    "request_id": "request-item"
                }),
                200,
            ),
            ResponseSpec::json(
                json!({
                    "removed": true,
                    "request_id": "request-remove"
                }),
                200,
            ),
        ],
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-item-remove");
    let config_path = temp_dir.join("config.json");
    let cache_path = cache_db_path(&config_path);
    StateStore::new(config_path.clone())
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let cache = PlaidCacheStore::open(&cache_path).expect("cache should open");
    cache
        .cache_item(&json!({
            "item_id": "item-remove",
            "institution_id": "ins_109508"
        }))
        .expect("item should cache");
    cache
        .cache_accounts(
            "item-remove",
            &[json!({
                "account_id": "acc-remove",
                "name": "Checking",
                "balances": { "available": 10.0 }
            })],
            AccountSnapshotSource::AccountsGet,
        )
        .expect("accounts should cache");
    cache
        .cache_transactions_sync(
            "item-remove",
            Some("acc-remove"),
            "cursor-remove",
            &[json!({
                "transaction_id": "tx-remove",
                "account_id": "acc-remove",
                "amount": 10.0,
                "name": "Disposable"
            })],
            &[],
            &[],
        )
        .expect("transactions should cache");

    let output: Value = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "item",
        "remove",
    ]);

    let requests = captured_requests(&capture);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/item/get");
    assert_eq!(requests[1].path, "/item/remove");
    let remove_body: Value = requests[1].json_body();
    assert_eq!(remove_body["access_token"], "stored-access-token");
    assert_eq!(output["removed"], true);
    assert_eq!(output["item_id"], "item-remove");
    assert_eq!(output["local_cache_purged"]["items_deleted"], 1);
    assert_eq!(output["local_cache_purged"]["accounts_deleted"], 1);
    assert_eq!(output["local_cache_purged"]["transactions_deleted"], 1);
    assert_eq!(output["local_cache_purged"]["cursors_deleted"], 1);
    assert_eq!(output["local_state"]["access_token_cleared"], true);
    assert_eq!(output["local_state"]["item_id_cleared"], true);

    let state = StateStore::new(config_path).load().expect("state should load");
    assert!(state.access_token.is_none());
    assert!(state.item_id.is_none());
    assert_eq!(row_count(&cache_path, "plaid_items", "item_id = 'item-remove'"), 0);
    assert_eq!(row_count(&cache_path, "plaid_accounts", "item_id = 'item-remove'"), 0);
    assert_eq!(
        row_count(&cache_path, "plaid_transactions", "item_id = 'item-remove'"),
        0
    );
    assert!(cached_cursor(&cache_path, "item-remove", Some("acc-remove")).is_none());
}

#[derive(Debug, Deserialize)]
struct ExchangePublicTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct LinkTokenCreateResponse {
    link_token: String,
}

#[derive(Debug, Deserialize)]
struct AccountsBalanceResponse {
    accounts: Vec<AccountSummary>,
}

#[derive(Debug, Deserialize)]
struct AccountSummary {
    account_id: String,
}

#[derive(Debug, Deserialize)]
struct TransactionsSyncResponse {
    next_cursor: String,
    pages_fetched: u64,
}

#[derive(Debug, Deserialize)]
struct JsonErrorResponse {
    kind: String,
    message: String,
}

fn run_command<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
    serde_json::from_value(value).expect("command output should match expected type")
}

fn run_command_error<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let error = run(cli).expect_err("command should fail");
    let rendered = render_cli_error(&error, compact);
    serde_json::from_str(&rendered).expect("error output should match expected type")
}

struct TestServer {
    address: String,
    _handle: thread::JoinHandle<()>,
}

impl TestServer {
    fn spawn(body: String, status_code: u16, capture: Option<Arc<Mutex<Vec<CapturedRequest>>>>) -> Self {
        Self::spawn_sequence(vec![ResponseSpec { body, status_code }], capture)
    }

    fn spawn_sequence(responses: Vec<ResponseSpec>, capture: Option<Arc<Mutex<Vec<CapturedRequest>>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("local addr should exist");

        let handle = thread::spawn(move || {
            for response in responses {
                if let Ok((mut stream, _)) = listener.accept() {
                    let request = read_request(&mut stream);
                    if let Some(capture) = capture.as_ref() {
                        if let Ok(mut guard) = capture.lock() {
                            guard.push(request);
                        }
                    }

                    let status_text = match response.status_code {
                        200 => "OK",
                        201 => "Created",
                        400 => "Bad Request",
                        401 => "Unauthorized",
                        _ => "OK",
                    };
                    let payload = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response.status_code,
                        status_text,
                        response.body.len(),
                        response.body
                    );
                    let _ = stream.write_all(payload.as_bytes());
                } else {
                    break;
                }
            }
        });

        Self {
            address: format!("http://{address}"),
            _handle: handle,
        }
    }

    fn base_url(&self) -> String {
        self.address.clone()
    }
}

struct ResponseSpec {
    body: String,
    status_code: u16,
}

impl ResponseSpec {
    fn json(body: Value, status_code: u16) -> Self {
        Self {
            body: body.to_string(),
            status_code,
        }
    }
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn parse(buffer: &[u8]) -> Self {
        let headers_end = find_headers_end(buffer).expect("request should include headers");
        let headers = String::from_utf8_lossy(&buffer[..headers_end]);
        let mut lines = headers.lines();
        let request_line = lines.next().expect("request line should exist");
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().expect("method should exist").to_owned();
        let path = request_parts
            .next()
            .expect("target should exist")
            .split('?')
            .next()
            .expect("path should exist")
            .to_owned();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();

        Self {
            method,
            path,
            headers,
            body: buffer[headers_end + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("request body should be valid json")
    }
}

fn captured_request(capture: &Arc<Mutex<Vec<CapturedRequest>>>) -> CapturedRequest {
    capture
        .lock()
        .expect("capture lock should work")
        .clone()
        .into_iter()
        .next()
        .expect("request should be captured")
}

fn captured_requests(capture: &Arc<Mutex<Vec<CapturedRequest>>>) -> Vec<CapturedRequest> {
    capture.lock().expect("capture lock should work").clone()
}

fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    loop {
        let bytes_read = stream.read(&mut temp).expect("request should read");
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..bytes_read]);

        if let Some(headers_end) = find_headers_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..headers_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);

            let total_length = headers_end + 4 + content_length;
            if buffer.len() >= total_length {
                break;
            }
        }
    }

    CapturedRequest::parse(&buffer)
}

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

fn cache_db_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .expect("config path should have a parent")
        .join("plaid-cache.sqlite3")
}

fn cached_item(path: &Path, item_id: &str) -> Value {
    let connection = Connection::open(path).expect("cache db should open");
    let data: String = connection
        .query_row(
            "SELECT data_json FROM plaid_items WHERE item_id = ?1",
            params![item_id],
            |row| row.get(0),
        )
        .expect("cached item should exist");
    serde_json::from_str(&data).expect("cached item json should parse")
}

fn cached_account(path: &Path, account_id: &str) -> Value {
    let connection = Connection::open(path).expect("cache db should open");
    let data: String = connection
        .query_row(
            "SELECT data_json FROM plaid_accounts WHERE account_id = ?1",
            params![account_id],
            |row| row.get(0),
        )
        .expect("cached account should exist");
    serde_json::from_str(&data).expect("cached account json should parse")
}

fn cached_transaction(path: &Path, transaction_id: &str) -> Value {
    let connection = Connection::open(path).expect("cache db should open");
    let data: String = connection
        .query_row(
            "SELECT data_json FROM plaid_transactions WHERE transaction_id = ?1",
            params![transaction_id],
            |row| row.get(0),
        )
        .expect("cached transaction should exist");
    serde_json::from_str(&data).expect("cached transaction json should parse")
}

fn cached_transaction_removed(path: &Path, transaction_id: &str) -> bool {
    let connection = Connection::open(path).expect("cache db should open");
    connection
        .query_row(
            "SELECT is_removed FROM plaid_transactions WHERE transaction_id = ?1",
            params![transaction_id],
            |row| row.get::<_, bool>(0),
        )
        .expect("cached removed transaction should exist")
}

fn cached_cursor(path: &Path, item_id: &str, account_scope: Option<&str>) -> Option<String> {
    let connection = Connection::open(path).expect("cache db should open");
    connection
        .query_row(
            "SELECT cursor FROM plaid_sync_cursors WHERE item_id = ?1 AND account_scope = ?2",
            params![item_id, account_scope.unwrap_or("")],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

fn row_count(path: &Path, table: &str, filter_sql: &str) -> i64 {
    let connection = Connection::open(path).expect("cache db should open");
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {filter_sql}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("row count query should succeed")
}

//! aginx-secret — the human/admin face of the secret sidecar (M36
//! agsecret; N5② 吸收改姓).
//!
//! `set` reads the value from STDIN, never from argv — argv is
//! world-readable through ps and lives in shell history. Output is the
//! D1 envelope on stdout (also the face the n5 suite parses); stderr
//! carries nothing by design.

use std::io::Read;

use serde_json::json;

use aginx_secret::client::{default_socket, request};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sock = default_socket();

    let (op, extra): (&str, Vec<&str>) = match args.first().map(String::as_str) {
        Some(a) if !a.starts_with('-') => {
            (a, args.iter().skip(1).map(String::as_str).collect())
        }
        _ => usage_exit(),
    };

    let req = match (op, extra.as_slice()) {
        ("set", [scope]) => {
            // value from stdin: `echo`/heredoc pipe, interactive hidden-ish
            // read — argv never carries it
            let mut value = String::new();
            if std::io::stdin().read_to_string(&mut value).is_err() || value.trim().is_empty() {
                agio::exit_fail(
                    agio::ErrorType::Usage,
                    "missing_value",
                    "set reads the value from stdin (argv leaks via ps)",
                );
            }
            json!({"op": "put", "scope": scope, "value": value.trim_end_matches('\n')})
        }
        ("get", [scope]) => json!({"op": "get", "scope": scope}),
        ("env", [name]) => json!({"op": "env", "name": name}),
        ("rm", [scope]) => json!({"op": "rm", "scope": scope}),
        ("list", []) => json!({"op": "list"}),
        ("sign", [scope, string]) => json!({"op": "sign", "scope": scope, "string": string}),
        _ => usage_exit(),
    };

    match request(&sock, &req) {
        Ok(env) => {
            let ok = env.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            agio::print(&env);
            if !ok {
                let ty = env
                    .pointer("/error/type")
                    .and_then(|v| v.as_str())
                    .and_then(|s| match s {
                        "usage" => Some(agio::ErrorType::Usage),
                        "not_found" => Some(agio::ErrorType::NotFound),
                        "io" => Some(agio::ErrorType::Io),
                        "state" => Some(agio::ErrorType::State),
                        "auth" => Some(agio::ErrorType::Auth),
                        _ => Some(agio::ErrorType::Internal),
                    })
                    .unwrap_or(agio::ErrorType::Internal);
                std::process::exit(ty.exit_code());
            }
        }
        Err(e) => {
            agio::exit_fail(
                agio::ErrorType::State,
                "sidecar_unreachable",
                &format!("{e} — is aginx-secretd running? (aginx-svc status aginx-secretd)"),
            )
        }
    }
}

fn usage_exit() -> ! {
    eprintln!("usage: aginx-secret set <scope>   (value on stdin)");
    eprintln!("       aginx-secret get <scope>");
    eprintln!("       aginx-secret env <name>");
    eprintln!("       aginx-secret rm <scope>");
    eprintln!("       aginx-secret list");
    eprintln!("       aginx-secret sign <scope> <string>");
    std::process::exit(2);
}

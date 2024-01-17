// TODO
// use std::io::Write;
//
// use std::sync::Arc;
// use std::time::{Duration, SystemTime};
//
// use anyhow::anyhow;
//
// use cap_async_std::ambient_authority;
// use cap_async_std::fs::Dir;
// use golem_common::model::{TemplateId, WorkerId};
// use golem_worker_executor_base::host::managed_stdio::ManagedStandardIo;
// use golem_worker_executor_base::model::InterruptKind;
// use golem_worker_executor_base::services::invocation_key::InvocationKeyServiceDefault;
// use tokio::task::JoinSet;
// use uuid::Uuid;
// use wasmtime_wasi::preview2::Table;
//
// use crate::preview2::wasi;
// use crate::preview2::wasi::clocks::monotonic_clock;
// use crate::preview2::wasi::io::{poll, streams};
// use crate::wasi_host::create_context;
// use crate::wasi_host::helpers::ctx::*;
// use crate::wasi_host::
// io::streams::{Input, Output};
// use crate::wasi_host::wasi_http::WasiHttp;
//
// struct WasiTestCtx {
//     table: Table,
//     ctx: WasiCtx,
//     http: WasiHttp,
//     stdio: ManagedStandardIo,
// }
//
// impl WasiTestCtx {
//     async fn new(args: &[impl AsRef<str>], env: &[(impl AsRef<str>, impl AsRef<str>)]) -> Self {
//         let temp_dir = tempfile::Builder::new().prefix("golem").tempdir().unwrap();
//         let root_dir = Dir::open_ambient_dir(&temp_dir.path(), ambient_authority())
//             .await
//             .unwrap();
//
//         let invocation_key_service = Arc::new(InvocationKeyServiceDefault::new());
//         let stdio = ManagedStandardIo::new(
//             WorkerId {
//                 template_id: TemplateId(Uuid::new_v4()),
//                 worker_name: "test".to_string(),
//             },
//             invocation_key_service,
//         );
//         create_context(
//             args,
//             env,
//             root_dir,
//             Input::from_standard_io(stdio.clone()),
//             Output::from_standard_io(stdio.clone()),
//             Box::new(|_duration| anyhow!(InterruptKind::Suspend)),
//             Duration::from_secs(10),
//             |ctx, table| {
//                 let http = WasiHttp::new();
//                 Self {
//                     table,
//                     ctx,
//                     http,
//                     stdio,
//                 }
//             },
//         )
//         .unwrap()
//     }
// }
//
// impl WasiView for WasiTestCtx {
//     fn table(&self) -> &Table {
//         &self.table
//     }
//
//     fn table_mut(&mut self) -> &mut Table {
//         &mut self.table
//     }
//
//     fn ctx(&self) -> &WasiCtx {
//         &self.ctx
//     }
//
//     fn ctx_mut(&mut self) -> &mut WasiCtx {
//         &mut self.ctx
//     }
//
//     fn http(&self) -> &WasiHttp {
//         &self.http
//     }
//
//     fn http_mut(&mut self) -> &mut WasiHttp {
//         &mut self.http
//     }
// }
//
// #[tokio::test]
// async fn read_stdin_fails() {
//     let args: &[String] = &[];
//     let env: &[(String, String)] = &[];
//     let mut ctx = WasiTestCtx::new(args, env).await;
//
//     let stdin = wasi::cli::stdin::Host::get_stdin(&mut ctx).await.unwrap();
//     let result = streams::HostInputStream::read(&mut ctx, stdin, 1u64).await;
//
//     assert!(result.is_err());
// }
//
// #[tokio::test]
// async fn waiting_for_monotonic_clock_does_not_block() {
//     async fn sleep2sec(n: usize) -> usize {
//         println!("{} start", n);
//         let args: &[String] = &[];
//         let env: &[(String, String)] = &[];
//         let mut ctx = WasiTestCtx::new(args, env).await;
//
//         // let now_ns = monotonic_clock::Host::now(&mut ctx).await.unwrap();
//         // let pollable = monotonic_clock::Host::subscribe(&mut ctx, now_ns + 2_000_000_000, true).await.unwrap();
//         let pollable = monotonic_clock::Host::subscribe_duration(&mut ctx, 2_000_000_000)
//             .await
//             .unwrap();
//         let _ = poll::Host::poll(&mut ctx, vec![pollable]).await.unwrap();
//         println!("{} stop", n);
//         n
//     }
//
//     const N: usize = 10;
//
//     let start = SystemTime::now();
//     let mut set = JoinSet::new();
//     for n in 0..N {
//         set.spawn(async move { sleep2sec(n).await });
//     }
//     let mut seen = [false; N];
//     while let Some(res) = set.join_next().await {
//         let idx = res.unwrap();
//         seen[idx] = true;
//     }
//
//     let end = SystemTime::now();
//     let elapsed = end.duration_since(start).unwrap();
//
//     assert!(elapsed.as_secs() < 5);
//     for seen_i in seen {
//         assert!(seen_i);
//     }
// }
//
// #[tokio::test]
// async fn poll_oneoff_multiple_clocks() {
//     let args: &[String] = &[];
//     let env: &[(String, String)] = &[];
//     let mut ctx = WasiTestCtx::new(args, env).await;
//
//     let now_ns = monotonic_clock::Host::now(&mut ctx).await.unwrap();
//     let p1 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 1_000_000_000)
//         .await
//         .unwrap();
//     let p2 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 500_000_000)
//         .await
//         .unwrap();
//     let p3 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 1_000)
//         .await
//         .unwrap();
//     let r1 = poll::Host::poll(&mut ctx, vec![p1, p2, p3]).await.unwrap();
//
//     let p1 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 1_000_000_000)
//         .await
//         .unwrap();
//     let p2 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 500_000_000)
//         .await
//         .unwrap();
//     let r2 = poll::Host::poll(&mut ctx, vec![p1, p2]).await.unwrap();
//
//     let p1 = monotonic_clock::Host::subscribe_instant(&mut ctx, now_ns + 1_000_000_000)
//         .await
//         .unwrap();
//     let r3 = poll::Host::poll(&mut ctx, vec![p1]).await.unwrap();
//
//     assert_eq!(r1, vec!(2));
//     assert_eq!(r2, vec!(1));
//     assert_eq!(r3, vec!(0));
// }
//
// #[tokio::test]
// async fn poll_oneoff_mixed_clock_and_stream() {
//     let args: &[String] = &[];
//     let env: &[(String, String)] = &[];
//     let mut ctx = WasiTestCtx::new(args, env).await;
//
//     let mut temp = tempfile::tempfile().unwrap();
//     temp.write_all(b"hello").unwrap();
//     let temp = cap_async_std::fs::File::from_std(temp.into());
//     let fstream = Input::new_file(temp, 0);
//
//     let is = ctx.table_mut().push(fstream).unwrap();
//     let p1 = monotonic_clock::Host::subscribe_duration(&mut ctx, 2_000_000_000)
//         .await
//         .unwrap();
//     let p2 = streams::HostInputStream::subscribe(&mut ctx, is)
//         .await
//         .unwrap();
//
//     let result = poll::Host::poll(&mut ctx, vec![p1, p2]).await.unwrap();
//     assert_eq!(result, vec!(1));
// }
//
// #[tokio::test]
// async fn read_stdin_provided_string() {
//     let args: &[String] = &[];
//     let env: &[(String, String)] = &[];
//     let mut ctx = WasiTestCtx::new(args, env).await;
//
//     ctx.stdio
//         .start_single_stdio_call("hello world".to_string())
//         .await;
//
//     let stdin = wasi::cli::stdin::Host::get_stdin(&mut ctx).await.unwrap();
//     let result1 = streams::HostInputStream::read(&mut ctx, stdin, 4u64)
//         .await
//         .unwrap();
//     let stdin = wasi::cli::stdin::Host::get_stdin(&mut ctx).await.unwrap();
//     let result2 = streams::HostInputStream::read(&mut ctx, stdin, 16u64)
//         .await
//         .unwrap();
//
//     let s1 = String::from_utf8(result1).unwrap();
//     let s2 = String::from_utf8(result2).unwrap();
//
//     assert_eq!(s1, "hell");
//     assert_eq!(s2, "o world");
// }

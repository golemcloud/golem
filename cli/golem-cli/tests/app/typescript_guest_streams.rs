use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use test_r::{inherit_test_dep, tag, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[tag(agents_guest_bridge)]
#[timeout("5 minutes")]
async fn test_ts_native_stream_guest_bridge_e2e() {
    native_stream_guest_bridge_e2e("run", Ok("20:ready")).await;
}

#[test]
#[tag(agents_guest_bridge)]
#[timeout("5 minutes")]
async fn test_ts_native_stream_forwarding_guest_bridge_e2e() {
    native_stream_guest_bridge_e2e("forwardUnread", Ok("9:ready")).await;
}

#[test]
#[tag(agents_guest_bridge)]
#[timeout("5 minutes")]
async fn test_ts_native_stream_producer_error_guest_bridge_e2e() {
    native_stream_guest_bridge_e2e("fatalProducer", Err("Invocation Failed")).await;
}

#[test]
#[tag(agents_guest_bridge)]
#[timeout("5 minutes")]
async fn test_ts_native_stream_pending_cancel_guest_bridge_e2e() {
    native_stream_guest_bridge_e2e("cancelPending", Ok("cancelled:ready")).await;
}

#[test]
#[tag(agents_guest_bridge)]
#[timeout("5 minutes")]
async fn test_ts_native_stream_result_items_guest_bridge_e2e() {
    native_stream_guest_bridge_e2e("recoverableItems", Ok("ok:1,err:recoverable,ok:2:ready")).await;
}

async fn native_stream_guest_bridge_e2e(method_name: &str, expected: Result<&str, &str>) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    fs::create_dir_all(ctx.cwd_path_join("ts-native-streams")).unwrap();
    ctx.cd("ts-native-streams");
    for component in ["provider", "consumer"] {
        let output = ctx
            .cli([
                flag::YES,
                cmd::NEW,
                ".",
                flag::TEMPLATE,
                "ts",
                flag::COMPONENT_NAME,
                &format!("ts-native-streams:{component}"),
            ])
            .await;
        assert!(output.success_or_dump());
    }
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: ts-native-streams
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          ts-native-streams:provider:
            dir: provider
            templates: ts
          ts-native-streams:consumer:
            dir: consumer
            templates: ts
            dependencies:
              agents:
                - ts-native-streams:provider/StreamProvider
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("provider/src/counter-agent.ts"), indoc! {r#"
        import { z } from 'zod';
        import { AgentStream, defineAgent, method, Result, s } from '@golemcloud/golem-ts-sdk';
        const item = z.object({ label: z.string(), count: z.number() });
        const itemStream = s.stream(item) as unknown as z.ZodType<AgentStream<z.infer<typeof item>>>;
        const bundle = z.object({ siblings: z.array(itemStream), optional: z.optional(itemStream) });
        export const StreamProvider = defineAgent({
          name: 'StreamProvider', id: { name: z.string() },
          methods: {
            echo: method({ input: { items: s.stream(item) }, returns: s.stream(item) }),
            nested: method({ input: { items: s.stream(s.stream(item)) }, returns: s.stream(s.stream(item)) }),
            forward: method({ input: { bundle }, returns: bundle }),
            produce: method({ input: {}, returns: s.stream(item) }),
            produceError: method({ input: {}, returns: s.stream(item) }),
            results: method({ input: {}, returns: s.stream(s.result(z.number(), z.string())) }),
            pending: method({ input: { items: s.stream(item) }, returns: z.string() }),
            status: method({ input: {}, returns: z.string() }),
          },
        });
        export const StreamProviderImpl = StreamProvider.implement({
          init: () => ({}),
          methods: {
            echo: ({ items }) => items,
            nested: ({ items }) => items,
            forward: ({ bundle }) => bundle,
            produce: () => AgentStream.from([{ label: 'remote', count: 9 }]),
            produceError: () => AgentStream.from((async function* () {
              yield { label: 'before-failure', count: 1 };
              throw new Error('ts-guest-producer-failed');
            })()),
            results: () => AgentStream.from<Result<number, string>>([
              Result.ok(1), Result.err('recoverable'), Result.ok(2),
            ]),
            async pending({ items }) {
              for await (const _ of items) { /* Drain before the pending wait. */ }
              await new Promise<void>((resolve) => setTimeout(resolve, 60_000));
              return 'unexpected-completion';
            },
            status: () => 'ready',
          },
        });
    "#}).unwrap();
    let tsconfig_path = ctx.cwd_path_join("consumer/tsconfig.json");
    let mut tsconfig: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig_path).unwrap()).unwrap();
    tsconfig["compilerOptions"]["paths"]["stream-provider-guest-client"] = serde_json::json!([
        "../golem-temp/bridge-sdk/ts/internal/stream-provider-guest-client/stream-provider-guest-client.ts"
    ]);
    tsconfig["include"].as_array_mut().unwrap().extend([
        serde_json::json!("src/**/*.ts"),
        serde_json::json!("../golem-temp/bridge-sdk/ts/internal/stream-provider-guest-client/*.ts"),
    ]);
    fs::write_str(
        tsconfig_path,
        serde_json::to_string_pretty(&tsconfig).unwrap(),
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("consumer/src/counter-agent.ts"), indoc! {r#"
        import { z } from 'zod';
        import { AgentStream, defineAgent, method } from '@golemcloud/golem-ts-sdk';
        import { StreamProvider } from 'stream-provider-guest-client';
        export const StreamConsumer = defineAgent({
          name: 'StreamConsumer', id: { name: z.string() },
          methods: {
            run: method({ input: {}, returns: z.string() }),
            forwardUnread: method({ input: {}, returns: z.string() }),
            fatalProducer: method({ input: {}, returns: z.string() }),
            recoverableItems: method({ input: {}, returns: z.string() }),
            cancelPending: method({ input: {}, returns: z.string() }),
          },
        });
        export const StreamConsumerImpl = StreamConsumer.implement({
          init: () => ({}),
          methods: {
            async run() {
              const a = StreamProvider.get('a');
              let count = 0;
              const local = () => AgentStream.from([{ label: 'local', count: 2 }]);
              for await (const item of await a.echo(local())) count += item.count;
              for await (const inner of await a.nested(AgentStream.from([local(), local()]))) {
                for await (const item of inner) count += item.count;
              }
              for (const size of [0, 1, 3]) {
                const bundle = await a.forward({
                  siblings: Array.from({ length: size }, local), optional: size === 0 ? undefined : local(),
                });
                for (const stream of [...bundle.siblings, ...(bundle.optional ? [bundle.optional] : [])]) {
                  for await (const item of stream) count += item.count;
                }
              }
              const untouched = local();
              try {
                await a.echo.abortable(AbortSignal.abort(new Error('pre-abort')), untouched);
                throw new Error('expected abort');
              } catch (error) {
                if (!(error instanceof Error) || error.message !== 'pre-abort') throw error;
              }
              for await (const item of await a.echo(untouched)) count += item.count;
              const early = await a.produce();
              for await (const _ of early) break;
              return `${count}:${await a.status()}`;
            },
            async forwardUnread() {
              const a = StreamProvider.get('forward-source');
              const b = StreamProvider.get('forward-target');
              let count = 0;
              // Forward the received endpoint to a distinct remote agent without reading it.
              for await (const item of await b.echo(await a.produce())) count += item.count;
              return `${count}:${await b.status()}`;
            },
            async fatalProducer() {
              const stream = await StreamProvider.get('fatal').produceError();
              for await (const _ of stream) { /* Fatal failure must reject, not reach EOF. */ }
              return 'unexpected-clean-eof';
            },
            async recoverableItems() {
              const provider = StreamProvider.get('recoverable');
              const observed: string[] = [];
              for await (const value of await provider.results()) {
                observed.push('ok' in value ? `ok:${value.ok}` : `err:${value.err}`);
              }
              return `${observed.join(',')}:${await provider.status()}`;
            },
            async cancelPending() {
              const controller = new AbortController();
              const reason = new Error('cancel-pending-guest-rpc');
              let drained!: () => void;
              const inputDrained = new Promise<void>((resolve) => { drained = resolve; });
              const input = AgentStream.from((async function* () {
                yield { label: 'handshake', count: 1 };
                drained();
              })());
              let settled = false;
              const pending = StreamProvider.get('pending').pending.abortable(controller.signal, input)
                .then(() => { settled = true; return 'unexpected-completion'; }, (error) => {
                  settled = true;
                  if (error !== reason) throw error;
                  return 'cancelled';
                });
              // The producer advances only after its first write is accepted. Abort
              // after this handshake, while the remote method is still awaiting.
              await Promise.race([
                inputDrained,
                pending.then(() => { throw new Error('RPC settled before input drained'); }),
              ]);
              if (settled) throw new Error('RPC was not pending at cancellation');
              controller.abort(reason);
              const outcome = await pending;
              if (outcome !== 'cancelled') throw new Error(outcome);
              // Cancellation does not promise immediate cleanup of the remote worker.
              return `${outcome}:${await StreamProvider.get('after-cancel').status()}`;
            },
          },
        });
    "#}).unwrap();
    let output = ctx.cli([cmd::BUILD]).await;
    assert!(output.success_or_dump());
    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "StreamConsumer(\"test\")",
            method_name,
        ])
        .await;
    match expected {
        Ok(value) => {
            assert!(output.success_or_dump());
            assert!(output.stdout_contains(value));
        }
        Err(error) => {
            assert!(
                !output.success(),
                "fatal producer failure was reported as clean EOF"
            );
            if !output.stderr_contains(error) {
                output.dump();
            }
            assert!(output.stderr_contains(error));
            assert!(!output.stdout_contains("unexpected-clean-eof"));
        }
    }
}

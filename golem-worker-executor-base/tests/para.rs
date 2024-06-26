use std::collections::HashMap;
use std::path::Path;

use golem_common::model::WorkerId;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;

use crate::common;

#[tokio::test]
#[tracing::instrument]
async fn para_contact_to_generator() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let (
        asset_generator_component_id,
        campaign_contact_component_id,
        campaign_manager_component_id,
        campaign_component_id,
    ) = tokio::join! {
        executor.store_component("asset-generator"),
        executor.store_component("campaign-contact"),
        executor.store_component("campaign-manager"),
        executor.store_component("campaign")
    };

    let env = {
        let mut env = HashMap::new();

        env.insert(
            "ASSET_GENERATOR_COMPONENT_ID".to_string(),
            asset_generator_component_id.to_string(),
        );

        env.insert(
            "CAMPAIGN_MANAGER_COMPONENT_ID".to_string(),
            campaign_manager_component_id.to_string(),
        );

        env.insert(
            "CAMPAIGN_CONTACT_COMPONENT_ID".to_string(),
            campaign_contact_component_id.to_string(),
        );

        env
    };
    tracing::error!("Starting campaign");

    let campaign_worker_id = executor
        .start_worker_with(&campaign_component_id, "campaign-id", vec![], env)
        .await;

    let _ = executor.log_output(&campaign_worker_id).await;

    let result = executor
        .invoke_and_await(
            &campaign_worker_id,
            "para:campaign/api.{start-campaign}",
            vec![
                Value::String("contacts-id".into()),
                Value::String("campaign-1".into()),
            ],
        )
        .await;

    let _ = executor.log_output(&WorkerId {
        component_id: asset_generator_component_id.clone(),
        worker_name: "campaign-id-one".to_string(),
    });

    let _ = executor.log_output(&WorkerId {
        component_id: campaign_manager_component_id.clone(),
        worker_name: "campaign-id".to_string(),
    });

    drop(executor);

    tracing::error!("DONE");

    println!("result: {:?}", result);

    // tracing::error!("STARTING CAMPAIGN WORKER");

    // let registry_worker_id = executor
    //     .start_worker_with(
    //         &campaign_contact_component_id,
    //         "campaign-contact-1",
    //         vec![],
    //         env,
    //     )
    //     .await;

    // tracing::error!("STARTED CAMPAIGN WORKER");

    // let _ = executor.log_output(&registry_worker_id).await;

    // let initialize_contact = executor
    //     .invoke_and_await(
    //         &registry_worker_id,
    //         "para:campaign-contact/api.{initialize}",
    //         vec![
    //             Value::String("nico".to_string()),
    //             Value::String("campaign-id".to_string()),
    //         ],
    //     )
    //     .await;

    // tracing::error!("initialize_contact: {:?}", initialize_contact);

    // let send_campaign = executor
    //     .invoke_and_await(
    //         &registry_worker_id,
    //         "para:campaign-contact/api.{send-campaign}",
    //         vec![],
    //     )
    //     .await;

    // tracing::error!("SEND CAMPAIGN");

    // let _ = executor.log_output(&WorkerId {
    //     component_id: asset_generator_component_id.clone(),
    //     worker_name: "nico-campaign-id".to_string(),
    // });

    // let _ = executor.log_output(&WorkerId {
    //     component_id: campaign_manager_component_id.clone(),
    //     worker_name: "campaign-id".to_string(),
    // });

    // drop(executor);

    // tracing::error!("DONE");

    // println!("initialize_contact: {:?}", initialize_contact);
    // println!("send_campaign: {:?}", send_campaign);
}

use golem_wasm_ast::analysis::AnalysisContext;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::DefaultAst;

#[test]
fn debug_para_wasm() {
    let wasm_component = Path::new("../test-components")
        .to_path_buf()
        // .join("campaign-contact-composed.wasm");
        .join("asset-generator.wasm");
    let module_bytes: Vec<u8> = std::fs::read(wasm_component).unwrap();
    let component: Component<DefaultAst> = Component::from_bytes(&module_bytes).unwrap();

    println!("component metadata {:?}", component.get_metadata());

    let state = AnalysisContext::new(component);
    let analysed_exports = state.get_top_level_exports().unwrap();
    println!("{analysed_exports:#?}");
}

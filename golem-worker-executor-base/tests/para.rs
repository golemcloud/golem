use std::collections::HashMap;

use golem_common::model::WorkerId;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;

use crate::common;

#[tokio::test]
#[tracing::instrument]
async fn para_contact_to_generator() {
    let context = common::TestContext::new();
    let executor = common::start(&context).await.unwrap();

    let campaign_contact_component_id = executor.store_component("campaign-contact-composed").await;
    let asset_generator_component_id = executor.store_component("asset-generator").await;

    let mut env = HashMap::new();

    env.insert(
        "ASSET_GENERATOR_COMPONENT_ID".to_string(),
        campaign_contact_component_id.to_string(),
    );

    let registry_worker_id = executor
        .start_worker_with(
            &campaign_contact_component_id,
            "campaign-contact-1",
            vec![],
            env,
        )
        .await;

    let _ = executor.log_output(&registry_worker_id).await;

    let initialize_contact = executor
        .invoke_and_await(
            &registry_worker_id,
            "para:campaign-contact/api/initialize",
            vec![
                Value::String("nico".to_string()),
                Value::String("campaign-id".to_string()),
            ],
        )
        .await;

    let send_campaign = executor
        .invoke_and_await(
            &registry_worker_id,
            "para:campaign-contact/api/send-campaign",
            vec![],
        )
        .await;

    let _ = executor.log_output(&WorkerId {
        component_id: asset_generator_component_id.clone(),
        worker_name: "nico-campaign-id".to_string(),
    });

    drop(executor);

    println!("initialize_contact: {:?}", initialize_contact);
    println!("send_campaign: {:?}", send_campaign);
}

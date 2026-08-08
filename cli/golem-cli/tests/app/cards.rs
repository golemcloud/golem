use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::model::text::card::{CardGetView, CardListView, CardRevokeResult};
use golem_client::model::StoredCard;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

fn card_id(card: &StoredCard) -> Uuid {
    match card {
        StoredCard::Concrete(card) => card.card_id,
        StoredCard::Polymorphic(card) => card.card_id,
    }
}

#[test]
#[timeout("10m")]
async fn card_management_works_against_a_live_server(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "card-management";
    let agent_name = "CounterAgent(\"cards\")";

    ctx.start_server().await;
    let output = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(app_name);
    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());
    let output = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent_name, "increment"])
        .await;
    assert!(output.success_or_dump());

    let output = ctx
        .cli([
            "card",
            cmd::LIST,
            "--include-agent-initials",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let account_cards = output
        .stdout_json::<CardListView>()
        .into_iter()
        .next()
        .expect("card list produced no JSON output");
    assert_eq!(account_cards.cards.len(), 1);
    let initial_card_id = card_id(&account_cards.cards[0]);

    let output = ctx
        .cli([
            "card",
            cmd::LIST,
            "--agent",
            agent_name,
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let wallet = output
        .stdout_json::<CardListView>()
        .into_iter()
        .next()
        .expect("agent wallet list produced no JSON output");
    assert!(
        wallet
            .cards
            .iter()
            .any(|card| card_id(card) == initial_card_id)
    );

    let initial_card_id_string = initial_card_id.to_string();
    let output = ctx
        .cli([
            "card",
            cmd::GET,
            &initial_card_id_string,
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let card = output
        .stdout_json::<CardGetView>()
        .into_iter()
        .next()
        .expect("card get produced no JSON output");
    assert_eq!(card_id(&card.0), initial_card_id);

    let output = ctx
        .cli([
            flag::YES,
            "card",
            "revoke",
            &initial_card_id_string,
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let revoked = output
        .stdout_json::<CardRevokeResult>()
        .into_iter()
        .next()
        .expect("card revoke produced no JSON output");
    assert_eq!(revoked.revoked_card_ids, vec![initial_card_id]);

    let output = ctx
        .cli([
            "card",
            cmd::LIST,
            "--include-agent-initials",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let account_cards = output
        .stdout_json::<CardListView>()
        .into_iter()
        .next()
        .expect("card list after revoke produced no JSON output");
    assert!(account_cards.cards.is_empty());
}

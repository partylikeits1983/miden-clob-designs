use tokio::time::{sleep, Duration};

use miden_client::{
    asset::FungibleAsset,
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt,
};

use std::time::{SystemTime, UNIX_EPOCH};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_option_contract_note, initialize_client,
    reset_store_sqlite, wait_for_notes,
};

#[tokio::test]
async fn option_contract_test() -> Result<(), ClientError> {
    reset_store_sqlite().await;

    let mut client = initialize_client().await?;
    println!(
        "Client initialized successfully. Latest block: {}",
        client.sync_state().await.unwrap().block_num
    );

    // -------------------------------------------------------------------------
    // STEP 1: Create accounts and deploy faucet
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let alice_account = create_basic_account(&mut client).await?;
    println!("Alice's account ID: {:?}", alice_account.id());
    let bob_account = create_basic_account(&mut client).await?;
    println!("Bob's account ID: {:?}", bob_account.id());

    println!("\nDeploying a new fungible faucet.");
    let faucet_a = create_basic_faucet(&mut client).await?;
    println!("Faucet account A ID: {:?}", faucet_a.id());
    let faucet_b = create_basic_faucet(&mut client).await?;
    println!("Faucet account B ID: {:?}", faucet_b.id());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint tokens with P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Mint tokens with P2ID");
    println!("mint for alice");
    let faucet_id = faucet_a.id();
    let amount: u64 = 100;
    let mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();
    let tx_req = TransactionRequestBuilder::mint_fungible_asset(
        mint_amount,
        alice_account.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build();
    let tx_exec = client.new_transaction(faucet_a.id(), tx_req).await?;
    client.submit_transaction(tx_exec.clone()).await?;

    let p2id_note = if let OutputNote::Full(note) = tx_exec.created_notes().get_note(0) {
        note.clone()
    } else {
        panic!("Expected OutputNote::Full");
    };

    sleep(Duration::from_secs(3)).await;
    wait_for_notes(&mut client, &alice_account, 1).await?;

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(p2id_note.id(), None)])
        .build();
    let tx_exec = client
        .new_transaction(alice_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;

    println!("mint for bob");
    let faucet_id = faucet_b.id();
    let amount: u64 = 100;
    let mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();
    let tx_req = TransactionRequestBuilder::mint_fungible_asset(
        mint_amount,
        bob_account.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build();
    let tx_exec = client.new_transaction(faucet_b.id(), tx_req).await?;
    client.submit_transaction(tx_exec.clone()).await?;

    let p2id_note = if let OutputNote::Full(note) = tx_exec.created_notes().get_note(0) {
        note.clone()
    } else {
        panic!("Expected OutputNote::Full");
    };

    sleep(Duration::from_secs(3)).await;
    wait_for_notes(&mut client, &bob_account, 1).await?;

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(p2id_note.id(), None)])
        .build();
    let tx_exec = client
        .new_transaction(bob_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;
    // -------------------------------------------------------------------------
    // STEP 3: Create custom note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create custom note");

    let offered_asset = FungibleAsset::new(faucet_a.id(), 100).unwrap();
    let requested_asset = FungibleAsset::new(faucet_b.id(), 100).unwrap();

    let now = SystemTime::now();
    let current_timestamp = now
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();
    let one_month: u64 = 60 * 60 * 24 * 30;
    let expiration = current_timestamp + one_month;

    let (option_contract_note, _p2id_payback_note_details) = create_option_contract_note(
        alice_account.id(),
        bob_account.id(),
        offered_asset.into(),
        requested_asset.into(),
        expiration,
        true,
        NoteType::Public,
        Felt::new(0),
        client.rng(),
    )
    .unwrap();

    println!(
        "option contract note: {:?}",
        option_contract_note.recipient().digest()
    );

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(option_contract_note.clone())])
        .unwrap()
        .build();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 4: Consume the Custom Note
    // -------------------------------------------------------------------------
    wait_for_notes(&mut client, &bob_account, 1).await?;
    println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(option_contract_note.id(), None)])
        .build();
    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();
    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    let _ = client.submit_transaction(tx_result).await;

    Ok(())
}

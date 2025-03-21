use tokio::time::{sleep, Duration};

use miden_client::{
    asset::FungibleAsset,
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt,
};

use miden_crypto::rand::FeltRng;

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_p2id_note, create_partial_swap_note,
    get_p2id_serial_num, get_swapp_note, initialize_client, reset_store_sqlite, wait_for_notes,
};

#[tokio::test]
async fn swap_note_partial_consume_test() -> Result<(), ClientError> {
    // reset store.sqlite file
    reset_store_sqlite().await;

    let mut client = initialize_client().await?;
    println!(
        "Client initialized successfully. Latest block: {}",
        client.sync_state().await.unwrap().block_num
    );

    // -------------------------------------------------------------------------
    // STEP 1: Create accounts and deploy faucets
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let alice_account = create_basic_account(&mut client).await?;
    println!("Alice's account ID: {:?}", alice_account.id());
    let bob_account = create_basic_account(&mut client).await?;
    println!("Bob's account ID: {:?}", bob_account.id());

    println!("\nDeploying two new fungible faucets.");
    let faucet_a = create_basic_faucet(&mut client).await?;
    println!("Faucet A account ID: {:?}", faucet_a.id());

    let faucet_b = create_basic_faucet(&mut client).await?;
    println!("Faucet B account ID: {:?}", faucet_b.id());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint Asset A & B tokens for Alice & Bob
    // -------------------------------------------------------------------------
    println!("\n[STEP 2]  Mint Asset A & B tokens for Alice & Bob");
    // Mint for Alice Asset A
    println!("mint for alice");
    let amount_a_alice_wallet: u64 = 100;
    let mint_amount_a = FungibleAsset::new(faucet_a.id(), amount_a_alice_wallet).unwrap();
    let tx_req = TransactionRequestBuilder::mint_fungible_asset(
        mint_amount_a,
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

    let _ = client.sync_state().await;

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(p2id_note.id(), None)])
        .build();
    let tx_exec = client
        .new_transaction(alice_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;

    // Mint for Bob Asset B
    println!("mint for bob");
    let amount_b_bob_wallet: u64 = 25;
    let mint_amount_b = FungibleAsset::new(faucet_b.id(), amount_b_bob_wallet).unwrap();
    let tx_req = TransactionRequestBuilder::mint_fungible_asset(
        mint_amount_b,
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

    let _ = client.sync_state().await;

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(p2id_note.id(), None)])
        .build();
    let tx_exec = client
        .new_transaction(bob_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 3: Create SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
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

    let swapp_note_id = swapp_note.id();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 4: Partial Consume SWAPP note
    // -------------------------------------------------------------------------

    // compute future output notes
    // exchange rate alice set of a:b is 1:1
    let amount_a_1 = 25;
    let asset_a_1 = FungibleAsset::new(faucet_a.id(), amount_a_1).unwrap();

    // bob sent 25 b tokens to alice
    let amount_b_1 = 25;
    let asset_b_1 = FungibleAsset::new(faucet_b.id(), amount_b_1).unwrap();

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        asset_a_1.into(),
        asset_b_1.into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // Build P2ID note
    let asset_amount_b_out = 25;
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), asset_amount_b_out).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);

    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    let _ = client.sync_state().await;

    // -------------------------------------------------------------------------
    // STEP 4: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    println!("CONSUMING NOTE");

    println!(
        "faucet A id: prefix: {:?} suffix: {:?}",
        faucet_a.id().prefix(),
        faucet_a.id().suffix()
    );
    println!(
        "faucet B id: prefix: {:?} suffix: {:?}",
        faucet_b.id().prefix(),
        faucet_b.id().suffix()
    );

    let _consume_amount_note_args = [Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(25)];

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), None)])
        .with_expected_output_notes(vec![p2id_note, swapp_note_1])
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

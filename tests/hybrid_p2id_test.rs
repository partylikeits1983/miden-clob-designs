use std::{fs, path::Path, sync::Arc};
use tokio::time::{sleep, Duration};

use miden_client::{
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_crypto::{hash::rpo::Rpo256 as Hasher, rand::FeltRng};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, delete_keystore_and_store, wait_for_notes,
};

#[tokio::test]
async fn hybrid_p2id_test_unlockable_succeeds() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    let endpoint = Endpoint::localhost();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    // -------------------------------------------------------------------------
    // STEP 1: Create accounts and deploy faucet
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let (alice_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());
    let (bob_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    println!("\nDeploying a new fungible faucet.");
    let faucet = create_basic_faucet(&mut client, keystore).await?;
    println!("Faucet account ID: {:?}", faucet.id().to_hex());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint tokens with P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Mint tokens with P2ID");
    let faucet_id = faucet.id();
    let amount: u64 = 100;
    let mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();
    let tx_req = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            mint_amount,
            alice_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();
    let tx_exec = client.new_transaction(faucet.id(), tx_req).await?;
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
        .build()
        .unwrap();
    let tx_exec = client
        .new_transaction(alice_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 3: Create Hybrid P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create Hybrid P2ID");

    let assembler = TransactionKernel::assembler().with_debug_mode(false);
    let code = fs::read_to_string(Path::new("./masm/notes/P2IDE.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();

    // Hybrid P2ID - P2ID(RT) Pay to Id, Optional Reclaimable & Timelockable

    let target = bob_account.id();
    let reclaim_block_height = Felt::new(0);
    let timelock_block_height = Felt::new(1);

    let note_inputs = NoteInputs::new(vec![
        target.suffix(),
        target.prefix().into(),
        reclaim_block_height,
        timelock_block_height,
    ])
    .unwrap();

    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs.clone());
    let tag: NoteTag = NoteTag::for_public_use_case(0, 0, NoteExecutionMode::Local).unwrap();
    let metadata = NoteMetadata::new(
        alice_account.id(),
        NoteType::Public,
        tag,
        NoteExecutionHint::always(),
        Felt::new(0),
    )?;
    let vault = NoteAssets::new(vec![mint_amount.into()])?;
    let custom_note = Note::new(vault, metadata, recipient);

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(custom_note.clone())])
        .build()
        .unwrap();
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

    println!("note inputs: {:?}", note_inputs.values());

    // -------------------------------------------------------------------------
    // STEP 4: Consume the Custom Note
    // -------------------------------------------------------------------------
    wait_for_notes(&mut client, &bob_account, 1).await?;
    println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(custom_note.id(), None)])
        .build()
        .unwrap();

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

#[tokio::test]
async fn hybrid_p2id_test_not_unlockable() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    // Initialize client
    let endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let unlock_block_height = sync_summary.block_num + 100;

    // -------------------------------------------------------------------------
    // STEP 1: Create accounts and deploy faucet
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let (alice_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());
    let (bob_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    println!("\nDeploying a new fungible faucet.");
    let faucet = create_basic_faucet(&mut client, keystore).await?;
    println!("Faucet account ID: {:?}", faucet.id().to_hex());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint tokens with P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Mint tokens with P2ID");
    let faucet_id = faucet.id();
    let amount: u64 = 100;
    let mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();
    let tx_req = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            mint_amount,
            alice_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();
    let tx_exec = client.new_transaction(faucet.id(), tx_req).await?;
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
        .build()
        .unwrap();
    let tx_exec = client
        .new_transaction(alice_account.id(), consume_req)
        .await?;
    client.submit_transaction(tx_exec).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 3: Create Hybrid P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create Hybrid P2ID");
    let mut secret_vals = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    secret_vals.splice(0..0, Word::default().iter().cloned());
    let digest = Hasher::hash_elements(&secret_vals);
    println!("digest: {:?}", digest);

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/HYBRID_P2ID.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();

    // Hybrid P2ID - P2ID(RT) Pay to Id, Optional Reclaimable & Timelockable

    let target = bob_account.id();
    let reclaim_block_height = Felt::new(0);
    let timelock_block_height = Felt::new(unlock_block_height.as_u64());

    let note_inputs = NoteInputs::new(vec![
        target.suffix(),
        target.prefix().into(),
        reclaim_block_height,
        timelock_block_height,
    ])
    .unwrap();

    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs.clone());
    let tag = NoteTag::for_public_use_case(0, 0, NoteExecutionMode::Local).unwrap();
    let metadata = NoteMetadata::new(
        alice_account.id(),
        NoteType::Public,
        tag,
        NoteExecutionHint::always(),
        Felt::new(0),
    )?;
    let vault = NoteAssets::new(vec![mint_amount.into()])?;
    let custom_note = Note::new(vault, metadata, recipient);
    println!("note hash: {:?}", custom_note.commitment());

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(custom_note.clone())])
        .build()
        .unwrap();
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

    println!("note inputs: {:?}", note_inputs.values());

    // -------------------------------------------------------------------------
    // STEP 4: Consume the Custom Note
    // -------------------------------------------------------------------------
    wait_for_notes(&mut client, &bob_account, 1).await?;
    println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(custom_note.id(), None)])
        .build()
        .unwrap();

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

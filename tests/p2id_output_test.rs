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
    ClientError, Felt,
};

use miden_crypto::rand::FeltRng;

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_p2id_note, delete_keystore_and_store,
    wait_for_notes,
};

#[tokio::test]
async fn p2id_output_test() -> Result<(), ClientError> {
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

    // -------------------------------------------------------------------------
    // STEP 1: Create accounts and deploy faucet
    // -------------------------------------------------------------------------
    println!("\n[STEP 1] Creating new accounts");
    let (alice_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());
    let (bob_account, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    println!("\nDeploying a new fungible faucet.");
    let faucet = create_basic_faucet(&mut client, keystore.clone()).await?;
    println!("Faucet account ID: {:?}", faucet.id());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint tokens with P2ID
    // -------------------------------------------------------------------------
    println!("\n[STEP 2] Mint tokens with P2ID");
    let faucet_id = faucet.id();
    let amount: u64 = 100;
    let mint_amount = FungibleAsset::new(faucet_id, amount).unwrap();

    let tx_req = TransactionRequestBuilder::mint_fungible_asset(
        mint_amount,
        alice_account.id(),
        NoteType::Public,
        client.rng(),
    )
    .unwrap()
    .build()
    .unwrap();
    let tx_exec = client.new_transaction(faucet.id(), tx_req).await?;
    client.submit_transaction(tx_exec.clone()).await?;

    let p2id_note = match tx_exec.created_notes().get_note(0) {
        OutputNote::Full(note) => note.clone(),
        _ => panic!("Expected OutputNote::Full"),
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
    // STEP 3: Create custom note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create custom note");

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/p2id_output_test.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();
    let note_inputs = NoteInputs::new(vec![]).unwrap();
    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs);
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
    client.submit_transaction(tx_result).await?;
    client.sync_state().await?;

    wait_for_notes(&mut client, &bob_account, 1).await?;

    // -------------------------------------------------------------------------
    // STEP 4: Consume the Custom Note
    // -------------------------------------------------------------------------
    let p2id_note_asset = FungibleAsset::new(faucet.id(), 50).unwrap();
    let p2id_serial_num = [Felt::new(1), Felt::new(1), Felt::new(1), Felt::new(1)];

    // Create the P2ID note that will be created in MASM.
    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num,
    )
    .unwrap();

    println!("p2id tag: {:?}", p2id_note.metadata().tag());
    println!("p2id aux: {:?}", p2id_note.metadata().aux());
    println!("p2id note type: {:?}", p2id_note.metadata().note_type());
    println!("p2id hint: {:?}", p2id_note.metadata().execution_hint());
    println!("recipient: {:?}", p2id_note.recipient().digest());
    println!("p2id asset: {:?}", p2id_note.assets());

    let note_tag_input: u64 = p2id_note.metadata().tag().into();
    println!("input tag: {:?}", note_tag_input);

    println!("\n[STEP 4] Bob consumes the Custom Note & Outputs P2ID Note for Alice");
    let note_args = [
        Felt::new(0),
        Felt::new(note_tag_input),
        alice_account.id().suffix(),
        alice_account.id().prefix().as_felt(),
    ];
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(custom_note.id(), Some(note_args))])
        .with_expected_output_notes(vec![p2id_note])
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
    client.submit_transaction(tx_result).await?;

    Ok(())
}

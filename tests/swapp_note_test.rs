use std::{fs, path::Path};
use tokio::time::{sleep, Duration};

use miden_client::{
    asset::FungibleAsset,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_crypto::{hash::rpo::Rpo256 as Hasher, rand::FeltRng};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_p2id_note, create_partial_swap_note,
    initialize_client, wait_for_notes,
};

#[tokio::test]
async fn swap_note_partial_consume_test() -> Result<(), ClientError> {
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
    println!("Alice's account ID: {:?}", alice_account.id().to_hex());
    let bob_account = create_basic_account(&mut client).await?;
    println!("Bob's account ID: {:?}", bob_account.id().to_hex());

    println!("\nDeploying two new fungible faucets.");
    let faucet_a = create_basic_faucet(&mut client).await?;
    println!("Faucet A account ID: {:?}", faucet_a.id().to_hex());

    let faucet_b = create_basic_faucet(&mut client).await?;
    println!("Faucet B account ID: {:?}", faucet_b.id().to_hex());
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Mint Asset A & B tokens for Alice & Bob
    // -------------------------------------------------------------------------
    println!("\n[STEP 2]  Mint Asset A & B tokens for Alice & Bob");
    // Mint for Alice Asset A
    let amount_a: u64 = 100;
    let mint_amount_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();
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
    let amount_b: u64 = 200;
    let mint_amount_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();
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
    println!("\n[STEP 3] Create custom note");

    let amount_a = 100;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    let amount_b = 200;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let fill_number = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        fill_number,
    )
    .unwrap();

    /*     let mut secret_vals = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    secret_vals.splice(0..0, Word::default().iter().cloned());
    let digest = Hasher::hash_elements(&secret_vals);
    println!("digest: {:?}", digest);

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/SWAPP.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();
    let note_inputs = NoteInputs::new(digest.to_vec()).unwrap();
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
    let swapp_note = Note::new(vault, metadata, recipient);
    println!("note hash: {:?}", swapp_note.hash()); */

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

    // -------------------------------------------------------------------------
    // STEP 4: Partial Consume SWAPP note
    // -------------------------------------------------------------------------

    // compute future output notes
    // exchange rate alice set of a:b is 1:2
    let amount_a_1 = amount_a - 50;
    let asset_a_1 = FungibleAsset::new(faucet_a.id(), amount_a_1).unwrap();

    // bob sent 100 b tokens to alice
    let amount_b_1 = amount_b - 100;
    let asset_b_1 = FungibleAsset::new(faucet_b.id(), amount_b_1).unwrap();

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let fill_number_1 = fill_number + 1;

    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        asset_a_1.into(),
        asset_b_1.into(),
        swap_serial_num_1,
        fill_number_1,
    )
    .unwrap();

    // Build P2ID note
    let asset_amount_b_out = 100;
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), asset_amount_b_out).unwrap();
    let p2id_serial_num_1 = client.rng().draw_word();
    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    // -------------------------------------------------------------------------
    // STEP 4: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    println!("CONSUMING NOTE");
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), None)])
        .with_expected_output_notes(vec![swapp_note_1, p2id_note])
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

    /*     wait_for_notes(&mut client, &bob_account, 1).await?;
       println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");
       let secret = [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
       let consume_custom_req = TransactionRequestBuilder::new()
           .with_authenticated_input_notes([(swapp_note.id(), Some(secret))])
           // .with_expected_output_notes(notes)
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
    */
    Ok(())
}

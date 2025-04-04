use std::{fs, path::Path, sync::Arc};

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
use miden_objects::crypto::hash::rpo::Rpo256;
use miden_objects::vm::AdviceMap;

use miden_clob_designs::common::{
    delete_keystore_and_store, setup_accounts_and_faucets, wait_for_notes,
};

#[tokio::test]
async fn hash_preimage_advice_provider() -> Result<(), ClientError> {
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

    // Setup accounts and balances
    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let faucet_a = faucets[0].clone();
    let _faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 3: Create custom note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create custom note");
    let mut secret_vals = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    secret_vals.splice(0..0, Word::default().iter().cloned());
    let digest = Hasher::hash_elements(&secret_vals);
    println!("digest: {:?}", digest);

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/advice_provider.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();
    let note_inputs = NoteInputs::new(digest.to_vec()).unwrap();
    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs.clone());
    let tag = NoteTag::for_public_use_case(0, 0, NoteExecutionMode::Local).unwrap();
    let metadata = NoteMetadata::new(
        alice_account.id(),
        NoteType::Public,
        tag,
        NoteExecutionHint::always(),
        Felt::new(0),
    )?;
    let asset_amount = FungibleAsset::new(faucet_a.id(), 100).unwrap();
    let vault = NoteAssets::new(vec![asset_amount.into()])?;
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

    let mut advice_map = AdviceMap::default();

    let note_args_value = vec![
        Felt::new(1),
        Felt::new(2),
        Felt::new(3),
        Felt::new(4),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
    ];

    let note_args_commitment = Rpo256::hash_elements(&note_args_value);
    println!("commitment: {:?}", note_args_commitment);

    advice_map.insert(note_args_commitment.into(), note_args_value.to_vec());

    let values_test = advice_map.get(&note_args_commitment);
    println!("sanity test: {:?}", values_test);

    wait_for_notes(&mut client, &bob_account, 1).await?;

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(custom_note.id(), Some(note_args_commitment.into()))])
        .extend_advice_map(advice_map)
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

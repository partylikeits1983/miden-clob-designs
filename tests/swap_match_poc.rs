use miden_client::{
    asset::FungibleAsset,
    keystore::FilesystemKeyStore,
    note::{NoteAssets, NoteType},
    rpc::Endpoint,
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt,
};
use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_exact_p2id_note, create_library_simplified,
    create_public_immutable_contract, create_public_note, create_tx_script,
    delete_keystore_and_store, instantiate_client, mint_from_faucet_for_matcher,
    setup_accounts_and_faucets, wait_for_note,
};
use miden_crypto::Word;
use miden_lib::note::{create_p2id_note, create_swap_note};
use std::{fs, path::Path};
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn increment_counter_with_note() -> Result<(), ClientError> {
    delete_keystore_and_store().await;

    let endpoint = Endpoint::testnet();
    let mut client = instantiate_client(endpoint).await?;
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    // -------------------------------------------------------------------------
    // STEP 1: Create Accounts (Alice, Bob, Matcher Account)
    // -------------------------------------------------------------------------
    // Setup accounts and balances
    let balances = vec![
        vec![100, 0],   // For account[0] => Alice
        vec![0, 100],   // For account[0] => Bob
        vec![100, 100], // For account[0] => matcher
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let matcher_account = accounts[2].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 2: Create the SWAP Notes & Expected P2ID output notes
    // -------------------------------------------------------------------------
    let asset_a = FungibleAsset::new(faucet_a.id(), 100).unwrap();
    let asset_b = FungibleAsset::new(faucet_b.id(), 100).unwrap();

    let (swap_note_1, p2id_details_1) = create_swap_note(
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        NoteType::Public,
        Felt::new(0),
        client.rng(),
    )
    .unwrap();
    let (swap_note_2, p2id_details_2) = create_swap_note(
        bob_account.id(),
        asset_b.into(),
        asset_a.into(),
        NoteType::Public,
        Felt::new(0),
        client.rng(),
    )
    .unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swap_note_2.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(bob_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let p2id_1 = create_exact_p2id_note(
        matcher_account.id(),
        alice_account.id(),
        vec![asset_b.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_details_1.serial_num(),
    )
    .unwrap();

    let p2id_2 = create_exact_p2id_note(
        matcher_account.id(),
        bob_account.id(),
        vec![asset_a.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_details_2.serial_num(),
    )
    .unwrap();

    // ---------------------------------------------------------------------------------
    // STEP 3: Consume both SWAP notes in a single TX by the matcher & output p2id notes
    // ---------------------------------------------------------------------------------
    let consume_custom_req = TransactionRequestBuilder::new()
        // .with_authenticated_input_notes([(swap_note_1.id(), None), (swap_note_1.id(), None)])
        .with_unauthenticated_input_notes([
            (swap_note_1.clone(), None),
            (swap_note_2.clone(), None),
        ])
        .with_expected_future_notes(vec![
            (p2id_1.clone().into(), p2id_1.metadata().tag()),
            (p2id_2.clone().into(), p2id_2.metadata().tag()),
        ])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(matcher_account.id(), consume_custom_req)
        .await
        .unwrap();
    let _ = client.submit_transaction(tx_result).await;

    client.sync_state().await.unwrap();


    

    Ok(())
}

use miden_objects::account::AccountComponent;
use rand::RngCore;
use std::{fs, path::Path, sync::Arc};
use tokio::time::{sleep, Duration};

use miden_client::{
    account::{AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageSlot},
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    rpc::{
        domain::account::{AccountStorageRequirements, StorageMapKey},
        Endpoint, TonicRpcClient,
    },
    transaction::{
        ForeignAccount, OutputNote, TransactionKernel, TransactionRequestBuilder, TransactionScript,
    },
    Client, ClientError, Felt, Word, ZERO,
};

use miden_crypto::rand::FeltRng;

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, create_library, delete_keystore_and_store,
    wait_for_notes,
};

/// Import the oracle + its publishers and return the ForeignAccount list
pub async fn get_oracle_foreign_accounts(
    client: &mut Client,
    oracle_account_id: AccountId,
) -> Result<Vec<ForeignAccount>, ClientError> {
    client.import_account_by_id(oracle_account_id).await?;

    let oracle_record = client
        .get_account(oracle_account_id)
        .await
        .expect("RPC failed")
        .expect("oracle account not found");

    let storage = oracle_record.account().storage();
    let publisher_count = storage.get_item(1).unwrap()[0].as_int();

    // collect publisher ids
    let publisher_ids: Vec<AccountId> = (1..publisher_count.saturating_sub(1))
        .map(|i| {
            let digest = storage.get_item(2 + i as u8).unwrap();
            let words: Word = digest.into();
            AccountId::new_unchecked([words[3], words[2]])
        })
        .collect();

    // build ForeignAccount list
    let mut foreign_accounts = Vec::with_capacity(publisher_ids.len() + 1);

    for pid in publisher_ids {
        client.import_account_by_id(pid).await?;

        foreign_accounts.push(ForeignAccount::public(
            pid,
            AccountStorageRequirements::new([(
                1u8,
                &[StorageMapKey::from([
                    ZERO,
                    ZERO,
                    ZERO,
                    Felt::new(120195681),
                ])],
            )]),
        )?);
    }

    // finally push the oracle itself
    foreign_accounts.push(ForeignAccount::public(
        oracle_account_id,
        AccountStorageRequirements::default(),
    )?);

    Ok(foreign_accounts)
}

#[tokio::test]
async fn oracle_test_account() -> Result<(), ClientError> {
    delete_keystore_and_store().await;

    // --- client init ---------------------------------------------------------
    let endpoint = Endpoint::testnet();
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, 10_000));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    println!("Latest block: {}", client.sync_state().await?.block_num);

    // --- import oracle + publishers -----------------------------------------
    let oracle_account_id = AccountId::from_hex("0x4f67e78643022e00000220d8997e33").unwrap();

    let foreign_accounts = get_oracle_foreign_accounts(&mut client, oracle_account_id).await?;

    // -------------------------------------------------------------------------
    // Create basic contract
    // -------------------------------------------------------------------------
    let contract_code =
        fs::read_to_string(Path::new("./masm/accounts/oracle_reader.masm")).unwrap();

    let assembler = TransactionKernel::assembler().with_debug_mode(true);

    let contract_component = AccountComponent::compile(
        contract_code.clone(),
        assembler,
        vec![StorageSlot::Value([
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
        ])],
    )
    .unwrap()
    .with_supports_all_types();

    let mut seed = [0_u8; 32];
    client.rng().fill_bytes(&mut seed);

    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    let (oracle_reader_contract, seed) = AccountBuilder::new(seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(contract_component.clone())
        .build()
        .unwrap();

    client
        .add_account(&oracle_reader_contract.clone(), Some(seed), false)
        .await
        .unwrap();

    // --- build & submit tx ---------------------------------------------------
    let script_path = Path::new("./masm/scripts/oracle_reader_script.masm");
    let script_code = fs::read_to_string(script_path).unwrap();

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let account_component_lib = create_library(
        assembler.clone(),
        "external_contract::oracle_reader",
        &contract_code,
    )
    .unwrap();

    let tx_script = TransactionScript::compile(
        script_code,
        [],
        assembler.with_library(&account_component_lib).unwrap(),
    )
    .unwrap();

    // Build a transaction request with the custom script
    let tx_increment_request = TransactionRequestBuilder::new()
        .with_foreign_accounts(foreign_accounts)
        .with_custom_script(tx_script)
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(oracle_reader_contract.id(), tx_increment_request)
        .await
        .unwrap();

    let tx_id = tx_result.executed_transaction().id();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    // Submit transaction to the network
    let _ = client.submit_transaction(tx_result).await;

    client.sync_state().await.unwrap();

    Ok(())
}

#[tokio::test]
async fn oracle_test_note() -> Result<(), ClientError> {
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
    let faucet = create_basic_faucet(&mut client, keystore).await?;
    println!("Faucet account ID: {:?}", faucet.id().to_hex());
    client.sync_state().await?;

    let oracle_account_id = AccountId::from_hex("0x4f67e78643022e00000220d8997e33").unwrap();
    client
        .import_account_by_id(oracle_account_id)
        .await
        .unwrap();

    println!(
        "prefix: {:?} suffix: {:?}",
        oracle_account_id.prefix(),
        oracle_account_id.suffix()
    );

    let publisher_account_id = AccountId::from_hex("0x0db5afa7f28ba90000029f98301f46").unwrap();
    client
        .import_account_by_id(publisher_account_id)
        .await
        .unwrap();

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
    // STEP 3: Create custom note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create custom note");

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/oracle_note.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();
    let note_inputs = NoteInputs::new(vec![].into()).unwrap();
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
    let oracle_foreign_account =
        ForeignAccount::public(oracle_account_id, AccountStorageRequirements::default()).unwrap();

    let publisher_foreign_account = ForeignAccount::public(
        publisher_account_id,
        AccountStorageRequirements::new([(
            1u8,
            &[StorageMapKey::from([
                ZERO,
                ZERO,
                ZERO,
                Felt::new(120195681),
            ])],
        )]),
    )
    .unwrap();

    wait_for_notes(&mut client, &bob_account, 1).await?;
    println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_foreign_accounts([publisher_foreign_account, oracle_foreign_account])
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

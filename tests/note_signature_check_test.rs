use miden_objects::vm::AdviceMap;
use std::{fs, path::Path, sync::Arc};
use tokio::time::{sleep, Duration};

use miden_client::{
    asset::FungibleAsset,
    builder::ClientBuilder,
    crypto::SecretKey,
    keystore::FilesystemKeyStore,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    ClientError, Felt, Word,
};

use miden_crypto::{dsa::rpo_falcon512::Polynomial, hash::rpo::Rpo256 as Hasher, rand::FeltRng};

use miden_clob_designs::common::{
    create_basic_account, create_basic_faucet, reset_store_sqlite, wait_for_notes,
};

const N: usize = 512;
fn mul_modulo_p(a: Polynomial<Felt>, b: Polynomial<Felt>) -> [u64; 1024] {
    let mut c = [0; 2 * N];
    for i in 0..N {
        for j in 0..N {
            c[i + j] += a.coefficients[i].as_int() * b.coefficients[j].as_int();
        }
    }
    c
}

fn to_elements(poly: Polynomial<Felt>) -> Vec<Felt> {
    poly.coefficients.to_vec()
}

fn generate_advice_stack_from_signature(h: Polynomial<Felt>, s2: Polynomial<Felt>) -> Vec<u64> {
    let pi = mul_modulo_p(h.clone(), s2.clone());

    // lay the polynomials in order h then s2 then pi = h * s2
    let mut polynomials = to_elements(h.clone());
    polynomials.extend(to_elements(s2.clone()));
    polynomials.extend(pi.iter().map(|a| Felt::new(*a)));

    // get the challenge point and push it to the advice stack
    let digest_polynomials = Hasher::hash_elements(&polynomials);
    let challenge = (digest_polynomials[0], digest_polynomials[1]);
    let mut advice_stack = vec![challenge.0.as_int(), challenge.1.as_int()];

    // push the polynomials to the advice stack
    let polynomials: Vec<u64> = polynomials.iter().map(|&e| e.into()).collect();
    advice_stack.extend_from_slice(&polynomials);

    advice_stack
}

#[tokio::test]
async fn falcon512_signature_check_note() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    reset_store_sqlite().await;

    // Initialize client
    let endpoint = Endpoint::testnet();
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
    let (alice_account, alice_key_pair) =
        create_basic_account(&mut client, keystore.clone()).await?;
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

    let alice_pub_key: Word = alice_key_pair.public_key().into();
    let note_inputs_1 = [
        alice_pub_key[0],
        alice_pub_key[1],
        alice_pub_key[2],
        alice_pub_key[3],
    ];

    // @dev if note_inputs_1 is used, this is the error: Number of account storage slots exceeds the maximum limit of 25
    let note_inputs = vec![
        alice_pub_key[0],
        alice_pub_key[1],
        alice_pub_key[2],
        Felt::new(4),
    ];

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let code = fs::read_to_string(Path::new("./masm/notes/SIG_CHECK.masm")).unwrap();
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(code, assembler).unwrap();
    let note_inputs = NoteInputs::new(note_inputs.to_vec()).unwrap();
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
    // STEP 4: Signing a hash
    // -------------------------------------------------------------------------
    let mut data = vec![Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    data.splice(0..0, Word::default().iter().cloned());
    let hashed_data = Hasher::hash_elements(&data);
    println!("digest: {:?}", hashed_data);

    // @dev make key pair used Alice's key pair
    let key_pair = alice_key_pair;

    // let h = key_pair.public_key().;
    let h = key_pair.compute_pub_key_poly().coefficients.clone();
    let h_poly = Polynomial::new(h).into();

    let signature = key_pair.sign(hashed_data.into());
    let s2: Polynomial<Felt> = signature.sig_poly().into();

    // generate_data_probabilistic_product_test()
    let advice_value_u64: Vec<u64> = generate_advice_stack_from_signature(h_poly, s2);
    let advice_value_felt: Vec<Felt> = advice_value_u64
        .into_iter()
        .map(|value| Felt::new(value))
        .collect();

    let mut advice_map = AdviceMap::default();

    let advice_key = Hasher::hash_elements(&signature.sig_poly().to_elements());

    advice_map.insert(advice_key.into(), advice_value_felt);

    println!("pub key felts: {:?}", key_pair.public_key());

    // -------------------------------------------------------------------------
    // STEP 4: Consume the Custom Note
    // -------------------------------------------------------------------------
    wait_for_notes(&mut client, &bob_account, 1).await?;
    println!("\n[STEP 4] Bob consumes the Custom Note with Correct Secret");
    // let secret = [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)];
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(custom_note.id(), Some(advice_key.into()))])
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

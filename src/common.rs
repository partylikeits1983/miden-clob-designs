use miden_assembly::{
    ast::{Module, ModuleKind},
    Assembler, DefaultSourceManager, LibraryPath,
};
use miden_crypto::dsa::rpo_falcon512::Polynomial;
use rand::{rngs::StdRng, RngCore};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::time::{sleep, Duration};

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
        Account, AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageSlot,
    },
    asset::{Asset, FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    crypto::{FeltRng, SecretKey},
    keystore::FilesystemKeyStore,
    note::{
        build_swap_tag, Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteId, NoteInputs,
        NoteMetadata, NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt, Word,
};
use miden_objects::{
    account::{AccountComponent, StorageMap},
    Hasher, NoteError,
};

// Signature verification code:

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

pub fn generate_advice_stack_from_signature(h: Polynomial<Felt>, s2: Polynomial<Felt>) -> Vec<u64> {
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

pub fn create_library(
    assembler: Assembler,
    library_path: &str,
    source_code: &str,
) -> Result<miden_assembly::Library, Box<dyn std::error::Error>> {
    let source_manager = Arc::new(DefaultSourceManager::default());
    let module = Module::parser(ModuleKind::Library).parse_str(
        LibraryPath::new(library_path)?,
        source_code,
        &source_manager,
    )?;
    let library = assembler.clone().assemble_library([module])?;
    Ok(library)
}

pub async fn create_basic_account(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<(miden_client::account::Account, SecretKey), ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = SecretKey::with_rng(client.rng());
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key().clone()))
        .with_component(BasicWallet);
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair.clone()))
        .unwrap();

    Ok((account, key_pair))
}

pub async fn create_basic_faucet(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = SecretKey::with_rng(client.rng());
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000);
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();
    Ok(account)
}

pub async fn create_signature_check_account(
    client: &mut Client,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let file_path = Path::new("./masm/accounts/sig_check_update.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    let empty_storage_slot = StorageSlot::empty_value();
    let storage_map = StorageMap::new();
    let storage_slot_map = StorageSlot::Map(storage_map.clone());

    let account_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![empty_storage_slot, storage_slot_map],
    )
    .unwrap()
    .with_supports_all_types();

    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(BasicWallet)
        .with_component(account_component);

    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;

    Ok(account)
}

pub async fn create_multisig_poc(
    client: &mut Client,
    authed_pub_keys: Vec<Word>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let file_path = Path::new("./masm/accounts/sig_check_update.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    let empty_storage_slot = StorageSlot::empty_value();
    // Declare storage_map as mutable
    let mut storage_map = StorageMap::new();

    let true_value = [Felt::new(1), Felt::new(1), Felt::new(1), Felt::new(1)];

    // Iterate over each key in the vector
    for key in authed_pub_keys.iter() {
        storage_map.insert(key.into(), true_value);
    }

    let storage_slot_map = StorageSlot::Map(storage_map.clone());

    let account_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![empty_storage_slot, storage_slot_map],
    )
    .unwrap()
    .with_supports_all_types();

    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(BasicWallet)
        .with_component(account_component);

    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;

    Ok(account)
}

/// Creates [num_accounts] accounts, [num_faucets] faucets, and mints the given [balances].
///
/// - `balances[a][f]`: how many tokens faucet `f` should mint for account `a`.
/// - Returns: a tuple of `(Vec<Account>, Vec<Account>)` i.e. (accounts, faucets).
pub async fn setup_accounts_and_faucets(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
    num_accounts: usize,
    num_faucets: usize,
    balances: Vec<Vec<u64>>,
) -> Result<(Vec<Account>, Vec<Account>), ClientError> {
    // 1) Create the [num_accounts] basic accounts
    let mut accounts = Vec::with_capacity(num_accounts);
    for i in 0..num_accounts {
        let (account, _) = create_basic_account(client, keystore.clone()).await?;
        println!("Created Account #{i} => ID: {:?}", account.id());
        accounts.push(account);
    }

    // 2) Create the [num_faucets] basic faucets
    let mut faucets = Vec::with_capacity(num_faucets);
    for j in 0..num_faucets {
        let faucet = create_basic_faucet(client, keystore.clone()).await?;
        println!("Created Faucet #{j} => ID: {:?}", faucet.id());
        faucets.push(faucet);
    }

    // Make sure the client has synced and sees these new accounts/faucets
    client.sync_state().await?;

    // 3) Mint tokens for each account from each faucet using `balances`
    //    Then consume each minted note, so the tokens truly reside in each account’s public vault
    for (acct_index, account) in accounts.iter().enumerate() {
        for (faucet_index, faucet) in faucets.iter().enumerate() {
            let amount_to_mint = balances[acct_index][faucet_index];
            if amount_to_mint == 0 {
                continue;
            }

            println!(
              "Minting {amount_to_mint} tokens from Faucet #{faucet_index} to Account #{acct_index}"
          );

            // Build a "mint fungible asset" transaction from this faucet
            let fungible_asset = FungibleAsset::new(faucet.id(), amount_to_mint).unwrap();
            let tx_req = TransactionRequestBuilder::mint_fungible_asset(
                fungible_asset,
                account.id(),
                NoteType::Public,
                client.rng(),
            )
            .unwrap()
            .build()
            .unwrap();

            // Submit the mint transaction
            let tx_exec = client.new_transaction(faucet.id(), tx_req).await?;
            client.submit_transaction(tx_exec.clone()).await?;

            // Extract the minted note
            let minted_note = if let OutputNote::Full(note) = tx_exec.created_notes().get_note(0) {
                note.clone()
            } else {
                panic!("Expected OutputNote::Full, but got something else");
            };

            // Wait for the note to appear on the account side
            wait_for_notes(client, account, 1).await?;
            client.sync_state().await?;

            // Now consume the minted note from the account side
            // so that the tokens reside in the account’s vault publicly
            let consume_req = TransactionRequestBuilder::new()
                .with_authenticated_input_notes([(minted_note.id(), None)])
                .build()
                .unwrap();

            let tx_exec = client.new_transaction(account.id(), consume_req).await?;
            client.submit_transaction(tx_exec).await?;
            client.sync_state().await?;
        }
    }

    Ok((accounts, faucets))
}

pub async fn wait_for_notes(
    client: &mut Client,
    account_id: &miden_client::account::Account,
    expected: usize,
) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;
        let notes = client.get_consumable_notes(Some(account_id.id())).await?;
        if notes.len() >= expected {
            break;
        }
        println!(
            "{} consumable notes found for account {}. Waiting...",
            notes.len(),
            account_id.id().to_hex()
        );
        sleep(Duration::from_secs(3)).await;
    }
    Ok(())
}

pub async fn get_swapp_note(
    client: &mut Client,
    tag: NoteTag,
    swapp_note_id: NoteId,
) -> Result<(), ClientError> {
    loop {
        // Sync the state and add the tag
        client.sync_state().await?;
        client.add_note_tag(tag).await?;

        // Fetch notes
        let notes = client.get_consumable_notes(None).await?;

        // Check if any note matches the swapp_note_id
        let found = notes.iter().any(|(note, _)| note.id() == swapp_note_id);

        if found {
            println!("Found the note with ID: {:?}", swapp_note_id);
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}

pub fn create_partial_swap_note(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "SWAPP.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.inner().into(),
        p2id_tag.into(),
        Felt::new(0),
        Felt::new(0),
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        creator.prefix().into(),
        creator.suffix().into(),
    ])?;

    let aux = Felt::new(0);

    // build the outgoing note
    let metadata = NoteMetadata::new(
        last_consumer,
        note_type,
        swapp_tag,
        NoteExecutionHint::always(),
        aux,
    )?;

    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(swap_serial_num, note_script.clone(), inputs.clone());
    let note = Note::new(assets.clone(), metadata, recipient.clone());

    Ok(note)
}

pub fn create_partial_swap_private_note(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "SWAPP_PRIVATE.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Private;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.inner().into(),
        p2id_tag.into(),
        Felt::new(0),
        Felt::new(0),
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        creator.prefix().into(),
        creator.suffix().into(),
    ])?;

    let aux = Felt::new(0);

    let metadata = NoteMetadata::new(
        last_consumer,
        note_type,
        swapp_tag,
        NoteExecutionHint::always(),
        aux,
    )?;

    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(swap_serial_num, note_script.clone(), inputs.clone());
    let note = Note::new(assets.clone(), metadata, recipient.clone());

    Ok(note)
}

pub fn create_partial_swap_note_cancellable(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    secret_hash: [Felt; 4],
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "SWAPP_cancellable.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.inner().into(),
        p2id_tag.into(),
        Felt::new(0),
        Felt::new(0),
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        creator.prefix().into(),
        creator.suffix().into(),
        Felt::new(0),
        Felt::new(0),
        secret_hash[0],
        secret_hash[1],
        secret_hash[2],
        secret_hash[3],
    ])?;

    let aux = Felt::new(0);

    // build the outgoing note
    let metadata = NoteMetadata::new(
        last_consumer,
        note_type,
        swapp_tag,
        NoteExecutionHint::always(),
        aux,
    )?;

    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(swap_serial_num, note_script.clone(), inputs.clone());
    let note = Note::new(assets.clone(), metadata, recipient.clone());

    println!(
        "inputlen: {:?}, NoteInputs: {:?}",
        inputs.num_values(),
        inputs.values()
    );
    println!("tag: {:?}", note.metadata().tag());
    println!("aux: {:?}", note.metadata().aux());
    println!("note type: {:?}", note.metadata().note_type());
    println!("hint: {:?}", note.metadata().execution_hint());
    println!("recipient: {:?}", note.recipient().digest());

    Ok(note)
}

pub fn create_p2id_note(
    sender: AccountId,
    target: AccountId,
    assets: Vec<Asset>,
    note_type: NoteType,
    aux: Felt,
    serial_num: [Felt; 4],
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "P2ID.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);

    let note_script = NoteScript::compile(note_code, assembler).unwrap();

    let inputs = NoteInputs::new(vec![target.suffix(), target.prefix().into()])?;
    let tag = NoteTag::from_account_id(target, NoteExecutionMode::Local)?;

    let metadata = NoteMetadata::new(sender, note_type, tag, NoteExecutionHint::always(), aux)?;
    let vault = NoteAssets::new(assets)?;

    let recipient = NoteRecipient::new(serial_num, note_script, inputs.clone());

    println!("inputs: {:?}", inputs.commitment());

    Ok(Note::new(vault, metadata, recipient))
}

pub async fn delete_keystore_and_store() {
    // Remove the SQLite store file
    let store_path = "./store.sqlite3";
    if tokio::fs::metadata(store_path).await.is_ok() {
        if let Err(e) = tokio::fs::remove_file(store_path).await {
            eprintln!("failed to remove {}: {}", store_path, e);
        } else {
            println!("cleared sqlite store: {}", store_path);
        }
    } else {
        println!("store not found: {}", store_path);
    }

    // Remove all files in the ./keystore directory
    let keystore_dir = "./keystore";
    match tokio::fs::read_dir(keystore_dir).await {
        Ok(mut dir) => {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let file_path = entry.path();
                if let Err(e) = tokio::fs::remove_file(&file_path).await {
                    eprintln!("failed to remove {}: {}", file_path.display(), e);
                } else {
                    println!("removed file: {}", file_path.display());
                }
            }
        }
        Err(e) => eprintln!("failed to read directory {}: {}", keystore_dir, e),
    }
}

pub fn get_p2id_serial_num(swap_serial_num: [Felt; 4], swap_count: u64) -> [Felt; 4] {
    let swap_count_word = [
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
    ];
    let p2id_serial_num = Hasher::merge(&[swap_serial_num.into(), swap_count_word.into()]);

    p2id_serial_num.into()
}

pub fn create_option_contract_note<R: FeltRng>(
    underwriter: AccountId,
    buyer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    expiration: u64,
    is_european: bool,
    aux: Felt,
    rng: &mut R,
) -> Result<(Note, Note), NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "option_contract_note.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));
    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let payback_serial_num = rng.draw_word();
    let p2id_note = create_p2id_note(
        buyer,
        underwriter,
        vec![requested_asset.into()],
        NoteType::Public,
        Felt::new(0),
        payback_serial_num,
    )
    .unwrap();

    let payback_recipient_word: Word = p2id_note.recipient().digest().into();
    let requested_asset_word: Word = requested_asset.into();
    let payback_tag = NoteTag::from_account_id(underwriter, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        payback_recipient_word[0],
        payback_recipient_word[1],
        payback_recipient_word[2],
        payback_recipient_word[3],
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        payback_tag.inner().into(),
        NoteExecutionHint::always().into(),
        underwriter.prefix().into(),
        underwriter.suffix().into(),
        buyer.prefix().into(),
        buyer.suffix(),
        Felt::new(expiration),
        Felt::new(is_european as u64),
    ])?;

    // build the tag for the SWAP use case
    let tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let serial_num = rng.draw_word();

    // build the outgoing note
    let metadata = NoteMetadata::new(
        underwriter,
        note_type,
        tag,
        NoteExecutionHint::always(),
        aux,
    )?;
    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(serial_num, note_script, inputs);
    let note = Note::new(assets, metadata, recipient);

    Ok((note, p2id_note))
}

/// Computes how many of the offered asset go out given `requested_asset_filled`,
/// then returns both the partial-fill amounts and the new remaining amounts.
///
/// Formulas:
///   amount_out = (offered_swapp_asset_amount * requested_asset_filled)
///                / requested_swapp_asset_amount
///
///   new_offered_asset_amount = offered_swapp_asset_amount - amount_out
///
///   new_requested_asset_amount = requested_swapp_asset_amount - requested_asset_filled
///
/// Returns a tuple of:
/// (amount_out, requested_asset_filled, new_offered_asset_amount, new_requested_asset_amount)
/// where:
///   - `amount_out` is how many of the offered asset will be sent out,
///   - `requested_asset_filled` is how many of the requested asset the filler provides,
///   - `new_offered_asset_amount` is how many of the offered asset remain unfilled,
///   - `new_requested_asset_amount` is how many of the requested asset remain unfilled.
pub fn compute_partial_swapp(
    offered_swapp_asset_amount: u64,
    requested_swapp_asset_amount: u64,
    requested_asset_filled: u64,
) -> (u64, u64, u64) {
    // amount of "offered" tokens (A) to send out
    let amount_out_offered = offered_swapp_asset_amount
        .saturating_mul(requested_asset_filled)
        .saturating_div(requested_swapp_asset_amount);

    // update leftover offered amount
    let new_offered_asset_amount = offered_swapp_asset_amount.saturating_sub(amount_out_offered);

    // update leftover requested amount
    let new_requested_asset_amount =
        requested_swapp_asset_amount.saturating_sub(requested_asset_filled);

    // Return partial fill info and updated amounts
    (
        amount_out_offered,
        new_offered_asset_amount,
        new_requested_asset_amount,
    )
}

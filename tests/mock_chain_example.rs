use miden_crypto::ONE;
use miden_tx::{testing::TransactionContextBuilder, TransactionInputs};

#[tokio::test]
async fn mockchain_test() {
    let tx_context = TransactionContextBuilder::with_standard_account(ONE).build();
    let code = "
        use.kernel::memory
        use.kernel::prologue

        begin
            exec.prologue::prepare_transaction
            exec.memory::get_blk_timestamp

            # truncate the stack
            swap drop
        end
    ";

    let process = tx_context.execute_code(code).unwrap();

    assert_eq!(
        process.stack.get(0),
        tx_context.tx_inputs().block_header().timestamp().into()
    );
    println!("stack state: {:?}", process.stack.get(0));
}

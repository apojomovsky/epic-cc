//! epic-cc#832: the runtime-`TBLRD` vtable slot load verifies in-sim over
//! entry `W` and entry `STATUS`, and its `mdb` batch assembles and fits.

use superopt::mdb::build_batch;
use superopt::specs::{tblrd_flash_ptr_candidate, tblrd_flash_ptr_nightly, tblrd_flash_ptr_pr};
use superopt::verify;

#[test]
fn tblrd_flash_ptr_reads_both_table_bytes() {
    assert!(verify(&tblrd_flash_ptr_candidate(), &tblrd_flash_ptr_pr()));
    assert!(verify(
        &tblrd_flash_ptr_candidate(),
        &tblrd_flash_ptr_nightly()
    ));
}

#[test]
fn tblrd_flash_ptr_batch_assembles_and_fits() {
    let batch = build_batch(&tblrd_flash_ptr_candidate(), &tblrd_flash_ptr_pr(), 0x100);
    let words = asm::assemble_pic18(&batch.src);
    assert!(!words.is_empty());
    assert_eq!(batch.reads.len(), batch.expected.len());
}

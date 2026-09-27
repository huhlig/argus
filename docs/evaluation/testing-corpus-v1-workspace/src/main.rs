//! Seed `cli-without-external-test`: the binary's argument handling and exit codes are never
//! exercised by a test that runs the program.

use argus_testing_corpus_v1::inventory::Inventory;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut inventory = Inventory::default();
    for argument in std::env::args().skip(1) {
        let Some((item, quantity)) = argument.split_once('=') else {
            eprintln!("expected ITEM=QUANTITY, got `{argument}`");
            return ExitCode::from(2);
        };
        match quantity.parse() {
            Ok(quantity) => inventory.add(item, quantity),
            Err(_) => {
                eprintln!("invalid quantity `{quantity}`");
                return ExitCode::from(2);
            }
        }
    }
    println!("{}", inventory.total());
    ExitCode::SUCCESS
}

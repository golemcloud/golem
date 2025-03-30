use rib::RibResult;

pub trait ResultPrinter {
    fn print_rib_result(&self, result: &RibResult) -> Result<(), String>;
    fn print_interpreter_error(&self, error: &String) -> Result<(), String>;
}


pub struct DefaultResultPrinter;

impl ResultPrinter for DefaultResultPrinter {
    fn print_rib_result(&self, result: &RibResult) -> Result<(), String> {
        println!("{}", result);
        Ok(())
    }

    fn print_interpreter_error(&self, error: &String) -> Result<(), String> {
        eprintln!("Error: {}", error);
        Ok(())
    }
}
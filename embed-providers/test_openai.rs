use std::env;
use std::error::Error;

// Import the necessary crates
use embed_common::EmbeddingProvider;
use embed_openai::OpenAIProvider;

fn main() -> Result<(), Box<dyn Error>> {
    // Load environment variables from .env file
    dotenv::dotenv().ok();
    
    // Check if OpenAI API key is set
    match env::var("OPENAI_API_KEY") {
        Ok(key) => {
            println!("Found OpenAI API key: {}...", key.chars().take(5).collect::<String>() + "***");
        },
        Err(_) => {
            eprintln!("Error: OPENAI_API_KEY environment variable not set");
            eprintln!("Please set it in the .env file or as an environment variable");
            return Ok(());
        }
    }
    
    // Initialize the OpenAI provider
    println!("Initializing OpenAI provider...");
    let provider = OpenAIProvider::new()?;
    
    // Create a simple test input
    let texts = vec![
        "Hello, world!".to_string(),
        "This is a test of the OpenAI embedding provider.".to_string(),
    ];
    
    // Generate embeddings
    println!("Generating embeddings for {} texts...", texts.len());
    
    // Use tokio runtime to run the async function
    let rt = tokio::runtime::Runtime::new()?;
    let embeddings = rt.block_on(provider.generate_embeddings(texts))?;
    
    // Print results
    println!("Successfully generated {} embeddings!", embeddings.len());
    println!("First embedding dimensions: {}", embeddings[0].len());
    println!("Second embedding dimensions: {}", embeddings[1].len());
    
    // Print a sample of the first embedding
    println!("\nSample of first embedding vector:");
    for (i, value) in embeddings[0].iter().take(5).enumerate() {
        println!("  [{}]: {}", i, value);
    }
    
    println!("\nOpenAI embedding test completed successfully!");
    Ok(())
}
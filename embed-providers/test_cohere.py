import os
import sys
import json
import requests
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Get Cohere API key from environment
api_key = os.getenv('COHERE_API_KEY')
if not api_key:
    print("Error: COHERE_API_KEY environment variable not set")
    print("Please set it in the .env file or as an environment variable")
    sys.exit(1)

print(f"Found Cohere API key: {api_key[:5]}***")

# Define the API endpoint
url = "https://api.cohere.ai/v1/embed"

# Define the request headers
headers = {
    "Authorization": f"Bearer {api_key}",
    "Content-Type": "application/json",
    "Accept": "application/json"
}

# Define the request data
data = {
    "model": "embed-english-v3.0",
    "texts": ["Hello, world!", "This is a test of the Cohere embedding provider."],
    "input_type": "search_document"
}

print("\nSending request to Cohere API...")

# Send the request
response = requests.post(url, headers=headers, json=data)

# Check if the request was successful
if response.status_code == 200:
    result = response.json()
    embeddings = result.get("embeddings", [])
    
    print("\nSuccess! Generated embeddings:")
    print(f"Number of embeddings: {len(embeddings)}")
    print(f"First embedding dimensions: {len(embeddings[0])}")
    print(f"Second embedding dimensions: {len(embeddings[1])}")
    
    # Print a sample of the first embedding
    print("\nSample of first embedding vector:")
    for i, value in enumerate(embeddings[0][:5]):
        print(f"  [{i}]: {value}")
    
    print("\nCohere embedding test completed successfully!")
else:
    print(f"\nError: {response.status_code}")
    print(response.text)
    print("\nCohere embedding test failed!")
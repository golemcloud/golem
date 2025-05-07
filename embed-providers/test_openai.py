import os
import sys
import json
import requests
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Get OpenAI API key from environment
api_key = os.getenv('OPENAI_API_KEY')
if not api_key:
    print("Error: OPENAI_API_KEY environment variable not set")
    print("Please set it in the .env file or as an environment variable")
    sys.exit(1)

print(f"Found OpenAI API key: {api_key[:5]}***")

# Define the API endpoint
url = "https://api.openai.com/v1/embeddings"

# Define the request headers
headers = {
    "Authorization": f"Bearer {api_key}",
    "Content-Type": "application/json"
}

# Define the request data
data = {
    "model": "text-embedding-3-large",
    "input": ["Hello, world!", "This is a test of the OpenAI embedding provider."],
    "encoding_format": "float"
}

print("\nSending request to OpenAI API...")

# Send the request
response = requests.post(url, headers=headers, json=data)

# Check if the request was successful
if response.status_code == 200:
    result = response.json()
    embeddings = [item["embedding"] for item in result["data"]]
    
    print("\nSuccess! Generated embeddings:")
    print(f"Number of embeddings: {len(embeddings)}")
    print(f"First embedding dimensions: {len(embeddings[0])}")
    print(f"Second embedding dimensions: {len(embeddings[1])}")
    
    # Print a sample of the first embedding
    print("\nSample of first embedding vector:")
    for i, value in enumerate(embeddings[0][:5]):
        print(f"  [{i}]: {value}")
    
    print("\nOpenAI embedding test completed successfully!")
else:
    print(f"\nError: {response.status_code}")
    print(response.text)
    print("\nOpenAI embedding test failed!")
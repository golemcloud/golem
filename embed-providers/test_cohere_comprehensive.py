#!/usr/bin/env python3

import os
import sys
import json
import requests
from dotenv import load_dotenv
from datetime import datetime

# Print header
print("\n" + "=" * 60)
print("Cohere Embedding Provider Test")
print("=" * 60)
print(f"Test run at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
print("-" * 60 + "\n")

# Load environment variables from .env file
print("Loading environment variables...")
load_dotenv()

# Get Cohere API key from environment
api_key = os.getenv('COHERE_API_KEY')
if not api_key:
    print("❌ Error: COHERE_API_KEY environment variable not set")
    print("   Please set it in the .env file or as an environment variable")
    sys.exit(1)

print(f"✅ Found Cohere API key: {api_key[:5]}***")

# Define the API endpoint
url = "https://api.cohere.ai/v1/embed"
model = "embed-english-v3.0"

# Define the request headers
headers = {
    "Authorization": f"Bearer {api_key}",
    "Content-Type": "application/json",
    "Accept": "application/json"
}

# Define test inputs
test_inputs = [
    "Hello, world!",
    "This is a test of the Cohere embedding provider.",
    "Embeddings are vector representations of text that capture semantic meaning."
]

# Define the request data
data = {
    "model": model,
    "texts": test_inputs,
    "input_type": "search_document"
}

print(f"\n📋 Test Configuration:")
print(f"   - Model: {model}")
print(f"   - Number of inputs: {len(test_inputs)}")
print(f"   - API endpoint: {url}")

print("\n🔄 Sending request to Cohere API...")

try:
    # Send the request
    response = requests.post(url, headers=headers, json=data)

    # Check if the request was successful
    if response.status_code == 200:
        result = response.json()
        embeddings = result.get("embeddings", [])
        
        print("\n✅ Success! Generated embeddings:")
        print(f"   - Number of embeddings: {len(embeddings)}")
        
        # Print dimensions for each embedding
        for i, embedding in enumerate(embeddings):
            print(f"   - Embedding {i+1} dimensions: {len(embedding)}")
        
        # Print a sample of each embedding
        print("\n🔍 Sample of embedding vectors (first 5 values):")
        for i, embedding in enumerate(embeddings):
            print(f"\n   Embedding {i+1} (for text: '{test_inputs[i][:30]}...'):")
            for j, value in enumerate(embedding[:5]):
                print(f"     [{j}]: {value}")
        
        # Print meta information if available
        if "meta" in result:
            meta = result["meta"]
            print(f"\n📊 Meta Information:")
            print(f"   - Billed characters: {meta.get('billed_units', {}).get('input_tokens', 'N/A')}")
            if "api_version" in meta:
                print(f"   - API version: {meta.get('api_version', 'N/A')}")
        
        print("\n✅ Cohere embedding test completed successfully!")
    else:
        print(f"\n❌ Error: HTTP {response.status_code}")
        try:
            error_data = response.json()
            print(f"   Error message: {error_data.get('message', 'Unknown error')}")
        except:
            print(f"   Response: {response.text}")
        print("\n❌ Cohere embedding test failed!")

except Exception as e:
    print(f"\n❌ Exception occurred: {str(e)}")
    print("\n❌ Cohere embedding test failed!")

print("\n" + "=" * 60 + "\n")
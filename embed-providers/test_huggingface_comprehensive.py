#!/usr/bin/env python3

import os
import sys
import json
import requests
from dotenv import load_dotenv
from datetime import datetime

# Print header
print("\n" + "=" * 60)
print("HuggingFace Embedding Provider Test")
print("=" * 60)
print(f"Test run at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
print("-" * 60 + "\n")

# Load environment variables from .env file
print("Loading environment variables...")
load_dotenv()

# Get HuggingFace API key from environment
api_key = os.getenv('HUGGINGFACE_API_KEY')
if not api_key:
    print("❌ Error: HUGGINGFACE_API_KEY environment variable not set")
    print("   Please set it in the .env file or as an environment variable")
    sys.exit(1)

print(f"✅ Found HuggingFace API key: {api_key[:5]}***")

# Define the API endpoint and model
model_id = "sentence-transformers/all-MiniLM-L6-v2"
url = f"https://api-inference.huggingface.co/pipeline/feature-extraction/{model_id}"

# Define the request headers
headers = {
    "Authorization": f"Bearer {api_key}",
    "Content-Type": "application/json"
}

# Define test inputs
test_inputs = [
    "Hello, world!",
    "This is a test of the HuggingFace embedding provider.",
    "Embeddings are vector representations of text that capture semantic meaning."
]

print(f"\n📋 Test Configuration:")
print(f"   - Model: {model_id}")
print(f"   - Number of inputs: {len(test_inputs)}")
print(f"   - API endpoint: {url}")

print("\n🔄 Sending requests to HuggingFace API...")

try:
    # HuggingFace API typically processes one input at a time
    # We'll send separate requests for each input
    embeddings = []
    
    for text in test_inputs:
        # Define the request data
        data = {
            "inputs": text,
            "options": {"wait_for_model": True}
        }
        
        # Send the request
        response = requests.post(url, headers=headers, json=data)
        
        # Check if the request was successful
        if response.status_code == 200:
            embedding = response.json()
            embeddings.append(embedding)
        else:
            print(f"\n❌ Error for input '{text[:30]}...': HTTP {response.status_code}")
            try:
                error_data = response.json()
                print(f"   Error message: {error_data.get('error', 'Unknown error')}")
            except:
                print(f"   Response: {response.text}")
            raise Exception(f"Failed to get embedding for input: {text[:30]}...")
    
    # Process results
    if len(embeddings) == len(test_inputs):
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
        
        print("\n✅ HuggingFace embedding test completed successfully!")
    else:
        print(f"\n❌ Error: Expected {len(test_inputs)} embeddings but got {len(embeddings)}")
        print("\n❌ HuggingFace embedding test failed!")

except Exception as e:
    print(f"\n❌ Exception occurred: {str(e)}")
    print("\n❌ HuggingFace embedding test failed!")

print("\n" + "=" * 60 + "\n")
import os
import sys
import json
import requests
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Get HuggingFace API key from environment
api_key = os.getenv('HUGGINGFACE_API_KEY')
if not api_key:
    print("Error: HUGGINGFACE_API_KEY environment variable not set")
    print("Please set it in the .env file or as an environment variable")
    sys.exit(1)

print(f"Found HuggingFace API key: {api_key[:5]}***")

# Define the API endpoint and model
model_id = "sentence-transformers/all-MiniLM-L6-v2"
url = f"https://api-inference.huggingface.co/pipeline/feature-extraction/{model_id}"

# Define the request headers
headers = {
    "Authorization": f"Bearer {api_key}",
    "Content-Type": "application/json"
}

print("\nSending requests to HuggingFace API...")

# HuggingFace API typically processes one input at a time
# We'll send separate requests for each input
texts = ["Hello, world!", "This is a test of the HuggingFace embedding provider."]
embeddings = []

try:
    for text in texts:
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
            print(f"\nError for input '{text[:30]}...': {response.status_code}")
            print(response.text)
            sys.exit(1)
    
    print("\nSuccess! Generated embeddings:")
    print(f"Number of embeddings: {len(embeddings)}")
    print(f"First embedding dimensions: {len(embeddings[0])}")
    print(f"Second embedding dimensions: {len(embeddings[1])}")
    
    # Print a sample of the first embedding
    print("\nSample of first embedding vector:")
    for i, value in enumerate(embeddings[0][:5]):
        print(f"  [{i}]: {value}")
    
    print("\nHuggingFace embedding test completed successfully!")
except Exception as e:
    print(f"\nException occurred: {str(e)}")
    print("\nHuggingFace embedding test failed!")
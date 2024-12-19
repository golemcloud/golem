import { NextRequest, NextResponse } from 'next/server';

export async function POST(request: NextRequest) {
  try {
    const { search } = new URL(request.url);
    const searchParams = new URLSearchParams(search);

    const path = searchParams.get("path");
    if (!path) {
      return NextResponse.json({ error: 'Missing "path" query parameter' }, { status: 400 });
    }

    const backendUrl = `http://localhost:9881/v1/${path}`;
    const headers: HeadersInit = Object.fromEntries(request.headers.entries());

    // Ensure necessary headers are included
    delete headers['host'];
    const requestBody = await request.json(); 
    // Parse the JSON body if it's expected
    const init: RequestInit = {
      method: request.method,
      headers: headers,
      body: JSON.stringify(requestBody),  // Ensure body is a JSON string
    };
    const backendResponse = await fetch(backendUrl, init);

    const result = await backendResponse.json();
    return  NextResponse.json(
      { status: backendResponse.status, data:  result}
    );
  } catch (error) {
    return NextResponse.json(
      { error: 'Unexpected error', details: (error as Error).message },
      { status: 500 }
    );
  }
}



export async function GET(request: NextRequest) {
  try {
    const { search } = new URL(request.url);
    const searchParams = new URLSearchParams(search);

    const backendUrl = `http://localhost:9881/v1/${searchParams.get("path")}`;
    const headers: HeadersInit = Object.fromEntries(request.headers.entries());
    delete headers['host'];
    const init: RequestInit = {
      method: request.method,
      headers: headers,
    };
    const backendResponse = await fetch(backendUrl, init);
    const result = await backendResponse.json();
    return  NextResponse.json(
      { status: backendResponse.status, data:  result}
    );
  } catch (error) {
    return NextResponse.json(
      { error: 'Error connecting to backend', details: (error as Error).message },
      { status: 500 }
    );
  }
}


//TODO need to add delte put other method


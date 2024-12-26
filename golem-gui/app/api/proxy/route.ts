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
    delete headers["content-length"]; // Fetch automatically calculates content-length
    // Parse the JSON body if it's expected
    interface ExtendedRequestInit extends RequestInit {
      duplex?: "half";
    }
    
    const init: ExtendedRequestInit = {
      method: request.method,
      headers: headers,
      body: request.body,
      duplex: "half",
    };
   
    const backendResponse = await fetch(backendUrl, init);
    const isJson = backendResponse.headers
      .get("content-type")
      ?.includes("application/json");

    const result = isJson
      ? await backendResponse.json()
      : await backendResponse.text();

    return  NextResponse.json(
      { status: backendResponse.status, data:  result},
      {status: backendResponse.status}
    );
  } catch (error) {
    console.log("error====>", error);

    return NextResponse.json(
      { error: 'Unexpected error', details: (error as Error).message },
      { status: 500 }
    );
  }
}
export async function PUT(request: NextRequest) {
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
    delete headers["content-length"]; // Fetch automatically calculates content-length
    // Parse the JSON body if it's expected
    interface ExtendedRequestInit extends RequestInit {
      duplex?: "half";
    }
    
    const init: ExtendedRequestInit = {
      method: request.method,
      headers: headers,
      body: request.body,
      duplex: "half",
    };
   
    const backendResponse = await fetch(backendUrl, init);
    const isJson = backendResponse.headers
      .get("content-type")
      ?.includes("application/json");

    const result = isJson
      ? await backendResponse.json()
      : await backendResponse.text();

    return  NextResponse.json(
      { status: backendResponse.status, data:  result},
      {status: backendResponse.status}
    );
  } catch (error) {
    console.log("error====>", error);

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

export async function DELETE(request: NextRequest) {
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
      { status: backendResponse.status, data:  result},
      {status: backendResponse.status}
    );
  } catch (error) {
    return NextResponse.json(
      { error: 'Error connecting to backend', details: (error as Error).message },
      { status: 500 }
    );
  }
}


//TODO need to add delte put other method


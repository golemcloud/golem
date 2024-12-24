import { Configuration, DefaultApi } from './';

// @ts-ignore
async function test() {
  const config = new Configuration({
    basePath: 'http://localhost:3000', // Base URL of the server
  });
  const api = new DefaultApi(config);

  try {
    // Test create user
    const newUser = {
      id: 1,
      name: 'Test User',
      email: 'test@example.com',
    };
    const createResponse = await api.createUser(newUser);
    console.assert(
        createResponse.status === 200,
        `Create user failed: ${createResponse.status}`
    );
    console.log('User successfully created:', createResponse.data);

    // Test get user
    const getResponse = await api.getUser(1);
    console.assert(
        getResponse.status === 200 && getResponse.data?.name === 'Test User',
        `Get user failed: ${JSON.stringify(getResponse.data)}`
    );
    console.log('User successfully retrieved:', getResponse.data);

    // Test update user
    const updatedUser = { ...newUser, name: 'Updated User' };
    const updateResponse = await api.updateUser(1, updatedUser);
    console.assert(
        updateResponse.status === 200 &&
        updateResponse.data?.name === 'Updated User',
        `Update user failed: ${JSON.stringify(updateResponse.data)}`
    );
    console.log('User successfully updated:', updateResponse.data);

    // Test delete user
    const deleteResponse = await api.deleteUser(1);
    console.assert(
        deleteResponse.status === 200,
        `Delete user failed: ${deleteResponse.status}`
    );
    console.log('User successfully deleted:', deleteResponse.data);
  } catch (error) {
    console.error('Error executing tests:', error);
  }
}

test().catch((err) => console.error('Error in the test:', err));

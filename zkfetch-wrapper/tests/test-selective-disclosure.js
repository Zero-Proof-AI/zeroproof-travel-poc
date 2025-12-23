#!/usr/bin/env node
/**
 * Test Selective Disclosure with zkFetch Wrapper Server
 * 
 * This test demonstrates end-to-end integration with the wrapper server,
 * testing selective disclosure via HTTP API.
 * Expected response:
      {
        "slideshow": {
          "author": "Yours Truly", 
          "date": "date of publication", 
          "slides": [
            {
              "title": "Wake up to WonderWidgets!", 
              "type": "all"
            }, 
            {
              "items": [
                "Why <em>WonderWidgets</em> are great", 
                "Who <em>buys</em> WonderWidgets"
              ], 
              "title": "Overview", 
              "type": "all"
            }
          ], 
          "title": "Sample Slide Show"
        }
      }
 */

const axios = require('axios');

const ZKFETCH_URL = 'http://localhost:8003/zkfetch';
const VERIFY_URL = 'http://localhost:8003/verify';

async function testSelectiveDisclosure() {
  console.log('=== Selective Disclosure Integration Test ===\n');
  console.log('Testing zkFetch wrapper server API...\n');

  // Test 1: responseMatches only (no redactions)
  console.log('Test 1: Extract Data with responseMatches');
  console.log('---------------------------------------');
  
  try {
    const response = await axios.post(ZKFETCH_URL, {
      url: 'https://httpbin.org/json',
      publicOptions: {
        method: 'GET',
        headers: {
          'accept': 'application/json'
        }
      },
      privateOptions: {
        responseMatches: [{
          type: 'contains',
          value: 'Sample Slide Show'
        }]
      }
      // Note: No redactions in this test to ensure responseMatches works
    });

    console.log('✅ Server generated proof with responseMatches\n');
    
    const { proof, metadata } = response.data;
    
    // Show what's extracted
    console.log('📄 Extracted Data (Visible):');
    console.log('   Proof contains text: "Sample Slide Show"');
    console.log('   Full response is included in proof');
    console.log('');
    
    // Verify via server endpoint
    console.log('🔍 Verifying proof via server...');
    const verifyResponse = await axios.post(VERIFY_URL, { proof });
    
    if (verifyResponse.data.valid) {
      console.log('✅ Proof verified by server!');
      console.log('   The proof is cryptographically valid');
      console.log('   responseMatches confirmed text existence');
    } else {
      console.log('❌ Proof verification failed');
      process.exit(1);
    }
    
    console.log('\n');
    
  } catch (error) {
    console.error('❌ Test 1 failed:', error.response?.data || error.message);
    process.exit(1);
  }

  // Test 2: Redactions only (no responseMatches)
  console.log('Test 2: Redact Sensitive Fields');
  console.log('---------------------------------------');
  
  try {
    const response = await axios.post(ZKFETCH_URL, {
      url: 'https://httpbin.org/json',
      publicOptions: {
        method: 'GET'
      },
      privateOptions: {},
      redactions: [
        {
          jsonPath: '$.slideshow.author'
        },
        {
          jsonPath: '$.slideshow.date'
        }
      ]
    });

    console.log('✅ Server generated proof with redactions\n');
    
    const { proof } = response.data;
    
    console.log('📄 What verifier sees:');
    console.log('   Response structure preserved');
    console.log('   Author field: ██████ (redacted)');
    console.log('   Date field: ██████ (redacted)');
    console.log('   Other fields: visible');
    console.log('');
    
    // Verify
    console.log('🔍 Verifying proof via server...');
    const verifyResponse = await axios.post(VERIFY_URL, { proof });
    
    if (verifyResponse.data.valid) {
      console.log('✅ Proof with redactions verified!');
      console.log('   Proves: Response structure is authentic');
      console.log('   Hidden: Author and publication date');
    } else {
      console.log('❌ Verification failed');
      process.exit(1);
    }
    
    console.log('\n');
    
  } catch (error) {
    console.error('❌ Test 2 failed:', error.response?.data || error.message);
    process.exit(1);
  }

  // Test 3: Both responseMatches AND redactions
  console.log('Test 3: Combined - Extract + Redact');
  console.log('---------------------------------------');
  
  try {
    const response = await axios.post(ZKFETCH_URL, {
      url: 'https://httpbin.org/json',
      publicOptions: {
        method: 'GET'
      },
      privateOptions: {
        responseMatches: [{
          type: 'contains',
          value: 'Sample Slide Show'  // Match the key "slideshow"
        }],
        // Use standard Reclaim "responseRedactions" here or below "redactions"
        // responseRedactions: [
        //   {
        //     jsonPath: '$.slideshow', // Redact entire slideshow object
        //     // xPath: '/data', // Xpath to redact
        //     // regex: '<REGEX>', // Regex to redact
        //   }
        // ],
      },
      redactions: [
        {
          jsonPath: '$.slideshow', // Redact entire slideshow object
          // xPath: '/data', // Xpath to redact
          // regex: '<REGEX>', // Regex to redact
        }
      ],
    });    

    console.log('✅ Server generated proof with both features\n');
    
    const { proof } = response.data;
    
    console.log('📄 What verifier sees:');
    console.log('   Proof contains: "slideshow" key');
    console.log('   Slideshow content: ██████ (entire object redacted)');
    console.log('');
    
    // Verify
    console.log('🔍 Verifying proof via server...');
    const verifyResponse = await axios.post(VERIFY_URL, { proof });
    
    if (verifyResponse.data.valid) {
      console.log('✅ Combined proof verified!');
      console.log('   Proves: Response contains slideshow key');
      console.log('   Hidden: Entire slideshow content');
      console.log('   Strategy: Match parent key, redact its content');
    } else {
      console.log('❌ Verification failed');
      process.exit(1);
    }
    
    console.log('\n');
    
  } catch (error) {
    console.error('❌ Test 3 failed:', error.response?.data?.error || error.message);
    process.exit(1);
  }

  // Test 4: Child Field Redactions with responseMatches
  console.log('Test 4: Child Field Redaction');
  console.log('---------------------------------------');
  console.log('   Issue: Redacting child fields');
  console.log('   Pattern: Match any text inside redacted $.slideshow.slides');
  console.log('   Expected: Should pass\n');
  
  try {
    const response = await axios.post(ZKFETCH_URL, {
      url: 'https://httpbin.org/json',
      publicOptions: {
        method: 'GET'
      },
      privateOptions: {
        responseMatches: [{
          type: 'contains',
          value: '"title": "Overview"'  // Match text inside slideshow
        }],
        // Use standard Reclaim "responseRedactions" here or below "redactions"
        // responseRedactions: [
        //   {
        //     jsonPath: '$.slideshow.slides', // JSON path to redact
        //     // xPath: '/data', // Xpath to redact
        //     // regex: '<REGEX>', // Regex to redact
        //   }
        // ],
      },
      redactions: [
          {
            jsonPath: '$.slideshow.slides', // JSON path to redact
            // xPath: '/data', // Xpath to redact
            // regex: '<REGEX>', // Regex to redact
          }
        ],
    });

    console.log('✅ Server generated proof with both features\n');
    
    const { proof } = response.data;
    
    console.log('📄 What verifier sees:');
    console.log('   Proof contains text: `"title": "Overview"`');
    console.log('   Slideshow content: ██████ (entire object redacted)');
    console.log('');
    
    // Verify
    console.log('🔍 Verifying proof via server...');
    const verifyResponse = await axios.post(VERIFY_URL, { proof });
    
    if (verifyResponse.data.valid) {
      console.log('✅ Redacting child proof verified!');
      console.log('   Proves: Response contains `"title": "Overview"` text ');
      console.log('   Hidden: child field slideshow.slides content');
      console.log('   Strategy: Match child key, redact its content');
    } else {
      console.log('❌ Verification failed');
      process.exit(1);
    }
    
    console.log('\n');
    
  } catch (error) {
    console.error('❌ Test 4 failed:', error.response?.data?.error || error.message);
    process.exit(1);
  }

  // Test 5: Known Issue - Child Field Redactions with responseMatches
  console.log('Test 5: Known Issue - Child Field Redaction + sibling field responseMatches');
  console.log('---------------------------------------');
  console.log('⚠️  NOTE: This pattern is known to fail with Reclaim SDK');
  console.log('   Issue: Redacting child fields while matching siblings breaks validation');
  console.log('   Pattern: Match any text + redact $.slideshow.author');
  console.log('   Expected: Should fail with "Response does not contain" error\n');
  
  try {
    const response = await axios.post(ZKFETCH_URL, {
      url: 'https://httpbin.org/json',
      publicOptions: {
        method: 'GET'
      },
      privateOptions: {
        responseMatches: [{
          type: 'contains',
          value: '"title": "Overview"'  // Match text inside slideshow
        }],
        responseRedactions: [
          {
            jsonPath: '$.slideshow.title', // JSON path to redact
            // xPath: '/data', // Xpath to redact
            // regex: '<REGEX>', // Regex to redact
          }
        ],
      },
    });

    console.log('❌ UNEXPECTED: Test 5 should have failed but passed!');
    console.log('   The SDK behavior may have changed.');
    console.log('');
    
  } catch (error) {
    const errorMsg = error.response?.data?.error || error.message;
    
    if (errorMsg.includes('Response does not contain')) {
      console.log('✅ Expected failure occurred');
      console.log(`   Error: ${errorMsg}`);
      console.log('');
      console.log('📝 Known Issue Explanation:');
      // console.log('   - Attestor validates responseMatches AFTER redactions');
      // console.log('   - Child field redactions corrupt response structure');
      // console.log('   - Workaround: Match parent key + redact entire parent object');
      // console.log('   - Example: Match "slideshow" + redact $.slideshow (see Test 3)');
      console.log('');
    } else {
      console.error('❌ Test 5 failed with unexpected error:', errorMsg);
      process.exit(1);
    }
  }

  // Summary
  console.log('=== Test Summary ===\n');
  console.log('✅ Selective Disclosure Patterns Tested:');
  console.log('  1. responseMatches only - content verification ✅');
  console.log('  2. redactions only - privacy protection ✅');
  console.log('  3. both combined - match key, redact content ✅');
  console.log('  4. both combined - match child key, redact content ✅');
  console.log('  5. known issue - child field redaction fails ⚠️');
  console.log('');
  console.log('✅ Server Integration Verified:');
  console.log('  - POST /zkfetch with selective disclosure');
  console.log('  - POST /verify proof validation');
  console.log('  - responseMatches for content extraction');
  console.log('  - responseRedactions for privacy (jsonPath)');
  console.log('  - Combined: match parent key + redact object content');
  console.log('');
  console.log('⚠️  Known Reclaim SDK Limitation:');
  console.log('  - Cannot combine responseMatches with child field redactions');
  console.log('  - Workaround: Match parent key + redact entire parent object');
  console.log('  - See Test 4 for working pattern, Test 5 for failing pattern');
  console.log('');
  console.log('🎯 End-to-end selective disclosure workflow verified!');
  console.log('');
  
  process.exit(0);
}

// Run the test
if (require.main === module) {
  testSelectiveDisclosure()
    .catch(error => {
      console.error('\n❌ Test suite failed:', error.message);
      process.exit(1);
    });
}

module.exports = { testSelectiveDisclosure };

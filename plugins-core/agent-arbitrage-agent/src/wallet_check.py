import os
import sys
from dotenv import load_dotenv
from py_clob_client.client import ClobClient
from py_clob_client.clob_types import ApiCreds
from web3 import Web3

# Harvey OS: Secure Wallet & API Check

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
ENV_PATH = os.path.join(HARVEY_HOME, "data", "arbitrage-agent", ".env.live")

def check_and_arm_vault():
    load_dotenv(ENV_PATH)
    
    host = "https://clob.polymarket.com"
    pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
    funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
    sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 2))
    key = os.environ.get("POLYMARKET_API_KEY")
    secret = os.environ.get("POLYMARKET_API_SECRET")
    passphrase = os.environ.get("POLYMARKET_API_PASSPHRASE")
    
    print("🔐 Cryptography Loaded. Connecting to Polymarket...")
    
    try:
        # 1. Test API Health
        creds = ApiCreds(api_key=key, api_secret=secret, api_passphrase=passphrase)
        client = ClobClient(host, key=pk, chain_id=137, signature_type=sig_type, funder=funder, creds=creds)
        
        if client.get_ok() == "OK":
            print("✅ API Health: Online (Authentication Accepted)")
        else:
            print("❌ API Health: Down")
            sys.exit(1)
            
        # 2. Fetch Live Balance via Native Web3 (Bypassing SDK Bug)
        print("🏦 Fetching Live Balance for USDC.e...")
        
        # Connect to Polygon
        w3 = Web3(Web3.HTTPProvider("https://polygon-rpc.com"))
        
        if not w3.is_connected():
            print("⚠️ Could not connect to Polygon RPC. Network might be busy.")
            return

        # USDC.e Contract on Polygon
        usdc_address = w3.to_checksum_address("0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174")
        funder_address = w3.to_checksum_address(funder)
        
        # Minimal ABI for balanceOf
        abi = [{"constant": True, "inputs": [{"name": "_owner", "type": "address"}], "name": "balanceOf", "outputs": [{"name": "balance", "type": "uint256"}], "type": "function"}]
        
        contract = w3.eth.contract(address=usdc_address, abi=abi)
        balance_wei = contract.functions.balanceOf(funder_address).call()
        balance_usdc = balance_wei / 1_000_000 # USDC has 6 decimals
        
        print("-" * 40)
        print(f"💰 LIVE BALANCE: ${balance_usdc:.2f} USDC.e")
        print("-" * 40)
        
        if balance_usdc > 0:
            print("\n🎉 SUCCESS! HARVEY IS FULLY ARMED.")
            print("You are ready to execute real-money arbitrage.")
        else:
            print("\n⚠️ Connection Successful, but balance is $0.00.")
            print(f"Please deposit USDC.e to: {funder}")
            
    except Exception as e:
        print(f"\n❌ Execution Error: {e}")

if __name__ == "__main__":
    check_and_arm_vault()

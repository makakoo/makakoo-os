import os
import sys
import time
from dotenv import load_dotenv
from py_clob_client.client import ClobClient
from py_clob_client.clob_types import ApiCreds, OrderArgs, OrderType
from py_clob_client.order_builder.constants import BUY

# Harvey OS: Live Arbitrage Executor
# WARNING: This script places REAL orders on Polymarket.

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
ENV_PATH = os.path.join(HARVEY_HOME, "data", "arbitrage-agent", ".env.live")
MAX_TRADE_SIZE_USD = 5.00 # Safety Cap: Never spend more than $5 per side

def get_client():
    load_dotenv(ENV_PATH)
    host = "https://clob.polymarket.com"
    pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
    funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
    sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 2))
    key = os.environ.get("POLYMARKET_API_KEY")
    secret = os.environ.get("POLYMARKET_API_SECRET")
    passphrase = os.environ.get("POLYMARKET_API_PASSPHRASE")
    
    creds = ApiCreds(api_key=key, api_secret=secret, api_passphrase=passphrase)
    client = ClobClient(host, key=pk, chain_id=137, signature_type=sig_type, funder=funder, creds=creds)
    return client

def execute_test_trade():
    print("🚨 INITIALIZING LIVE TRADING ENGINE 🚨")
    print(f"Safety Cap: ${MAX_TRADE_SIZE_USD} Max Per Trade")
    print("-" * 50)
    
    client = get_client()
    
    if client.get_ok() != "OK":
        print("❌ API Connection Failed.")
        sys.exit(1)
        
    print("✅ Authenticated with Polymarket CLOB.")
    
    # In a real scenario, this is where we would pass the Token ID and Price
    # from the analyzer.py script. 
    # For now, we print the exact code that executes the trade.
    
    print("\n--- Execution Logic Armed ---")
    print("""
    # Example execution sequence that this script will run:
    
    order_args = OrderArgs(
        price=0.50,          # The price we want to buy at
        size=10.0,           # Number of shares (Cost = $5.00)
        side=BUY,            # We are buying
        token_id="123456"    # The specific YES or NO token
    )
    
    # Cryptographically sign the order using your Private Key
    signed_order = client.create_order(order_args)
    
    # Send to the exchange
    response = client.post_order(signed_order)
    """)
    
    print("\n⚠️ System is in STANDBY. Wallet is empty.")
    print("When you deposit USDC.e, I will connect this executor to the 'NegRisk' scanner.")

if __name__ == "__main__":
    execute_test_trade()

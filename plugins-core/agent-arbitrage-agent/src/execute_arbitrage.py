import os
import sys
import json
from dotenv import load_dotenv
from py_clob_client.client import ClobClient
from py_clob_client.clob_types import ApiCreds, OrderArgs
from py_clob_client.order_builder.constants import BUY

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
ENV_PATH = os.path.join(HARVEY_HOME, "data", "arbitrage-agent", ".env.live")
CANDIDATES_FILE = os.path.join(HARVEY_HOME, "data", "arbitrage-agent", "negrisk_opportunities.json")

def get_client():
    load_dotenv(ENV_PATH)
    host = "https://clob.polymarket.com"
    pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
    funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
    sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 0))
    
    # Start WITHOUT creds to force derivation
    client = ClobClient(host, key=pk, chain_id=137, signature_type=sig_type, funder=funder)
    
    # EXPLICITLY DERIVE CREDS
    print("🔐 Deriving fresh API session from Private Key...")
    creds = client.create_or_derive_api_creds()
    client.set_api_creds(creds)
    
    return client

def run_debug_execution():
    print("🚀 HARVEY OS: ARBITRAGE DEBUGGER V2 🚀")
    print("-" * 60)
    
    try:
        client = get_client()
        print("✅ Authentication Session Created.")
        
        # 1. VERIFY
        if client.get_ok() != "OK":
            print("❌ Server rejected derived session.")
            return

        # 2. SELECT TARGET
        with open(CANDIDATES_FILE, 'r') as f:
            candidates = json.load(f)
        
        active = [c for c in candidates if c['type'] != "NONE" and len(c['markets']) < 15]
        target = active[0]
        print(f"\n🎯 TARGET: {target['title']}")
        
        # 3. PLACE ONE SINGLE MICRO ORDER ($0.10)
        market = target['markets'][0]
        token = market['tokens'][1] if "SHORT" in target['type'] else market['tokens'][0]
        
        print(f"⚡ Attempting $0.10 MICRO-ORDER on: {market['q'][:40]}...")
        
        order_args = OrderArgs(
            price=0.10, 
            size=1.0,   
            side=BUY,
            token_id=token
        )
        signed = client.create_order(order_args)
        resp = client.post_order(signed)
        
        if resp.get('success'):
            print(f"🎉 SUCCESS! Order Placed. ID: {resp.get('orderID')}")
        else:
            print(f"❌ EXECUTION FAILED: {resp}")
            
    except Exception as e:
        print(f"❌ CRITICAL ERROR: {e}")

if __name__ == "__main__":
    run_debug_execution()

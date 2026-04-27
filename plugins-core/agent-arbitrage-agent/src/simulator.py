import requests
import json
import time
import os
from datetime import datetime

# Harvey OS: Arbitrage Simulator V3 (Bid Side / Mint & Dump)
# This version pulls the "Bids" to see if we can sell a set for > $1.00.

MARKETS_API = "https://gamma-api.polymarket.com/markets"
CLOB_BOOK_API = "https://clob.polymarket.com/book"

def get_best_bid(token_id):
    try:
        r = requests.get(CLOB_BOOK_API, params={"token_id": token_id})
        if r.status_code == 200:
            book = r.json()
            if book.get('bids'):
                return float(book['bids'][0]['price']) # Best price to sell at
    except: pass
    return None

def fetch_bid_side_markets():
    try:
        r = requests.get(MARKETS_API, params={"active": "true", "closed": "false", "limit": 20})
        if r.status_code == 200:
            raw = r.json()
            markets = []
            for m in raw:
                token_ids = m.get('clobTokenIds')
                if isinstance(token_ids, str): token_ids = json.loads(token_ids)
                if token_ids and len(token_ids) == 2:
                    yes_bid = get_best_bid(token_ids[0])
                    no_bid = get_best_bid(token_ids[1])
                    if yes_bid and no_bid:
                        markets.append({
                            "id": m['id'],
                            "question": m['question'],
                            "prices": [yes_bid, no_bid], # Storing Bids here
                            "liquidity": float(m.get('liquidityNum', 0))
                        })
            return markets
    except Exception as e:
        print(f"Error: {e}")
    return []

def run_simulation(duration_minutes=5):
    import strategy
    start_time = time.time()
    end_time = start_time + (duration_minutes * 60)
    print(f"🚀 Kicking off V3 {duration_minutes}m simulation (BID Side)...")
    
    total_theoretical_profit = 0.0
    trades = 0
    
    while time.time() < end_time:
        markets = fetch_bid_side_markets()
        for m in markets:
            decision = strategy.evaluate_market(m)
            if decision.get('action') == 'MINT_AND_DUMP':
                edge = sum(m['prices']) - 1.0
                total_theoretical_profit += edge
                trades += 1
                print(f"[{datetime.now().strftime('%H:%M:%S')}] MINT-ARB: {m['question'][:30]}... Edge: {edge*100:.3f}%")
        
        time.sleep(15)
        print(".", end="", flush=True)
        
    hourly_yield = total_theoretical_profit * (60 / duration_minutes)
    
    results = {
        "timestamp": datetime.now().isoformat(),
        "strategy_version": getattr(strategy, "VERSION", "V1.3.0"),
        "total_trades": trades,
        "hourly_yield": hourly_yield,
        "score": (total_theoretical_profit)
    }
    
    with open("history.jsonl", "a") as f:
        f.write(json.dumps(results) + "\n")
        
    print(f"\n\n--- V3 SIM COMPLETE ---")
    print(f"Trades: {trades} | Yield: ${hourly_yield:.4f}")
    return results

if __name__ == "__main__":
    run_simulation(duration_minutes=2)

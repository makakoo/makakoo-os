import sys
from web3 import Web3

# Connect to a more reliable public Polygon RPC
POLYGON_RPC_URL = "https://rpc.ankr.com/polygon"
w3 = Web3(Web3.HTTPProvider(POLYGON_RPC_URL))

if not w3.is_connected():
    print("❌ Failed to connect to Polygon network")
    sys.exit(1)

TARGET_ADDRESS = "0xF8023650E438C1493059d41833EeD7497D20687b"
target_address = w3.to_checksum_address(TARGET_ADDRESS)

print(f"🔍 Scanning Blockchain for Assets in Wallet: {target_address}\n")

# 1. Native Token (POL / MATIC)
matic_balance_wei = w3.eth.get_balance(target_address)
matic_balance = w3.from_wei(matic_balance_wei, 'ether')
if matic_balance > 0:
    print(f"🪙  POL (MATIC): {matic_balance:.4f}")

# 2. ERC-20 Tokens (USDC.e and USDC)
ERC20_ABI = [
    {
        "constant": True,
        "inputs": [{"name": "_owner", "type": "address"}],
        "name": "balanceOf",
        "outputs": [{"name": "balance", "type": "uint256"}],
        "type": "function",
    },
    {
        "constant": True,
        "inputs": [],
        "name": "decimals",
        "outputs": [{"name": "", "type": "uint8"}],
        "type": "function",
    }
]

tokens = {
    "USDC.e (Bridged)": "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174",
    "USDC (Native)": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
    "WETH": "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619",
    "USDT": "0xc2132D05D31c914a87C6611C10748AEb04B58e8F"
}

found_assets = False

for name, address in tokens.items():
    contract_address = w3.to_checksum_address(address)
    contract = w3.eth.contract(address=contract_address, abi=ERC20_ABI)
    
    try:
        balance_raw = contract.functions.balanceOf(target_address).call()
        if balance_raw > 0:
            decimals = contract.functions.decimals().call()
            balance = balance_raw / (10 ** decimals)
            print(f"💵 {name}: {balance:.4f}")
            found_assets = True
    except Exception as e:
        print(f"Error checking {name}: {e}")

if not found_assets and matic_balance == 0:
    print("⚠️  Wallet is currently empty on the Polygon network.")
else:
    print("\n✅ Verification complete. Harvey has direct blockchain visibility.")

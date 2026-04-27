"""
Configuration for Polymarket Trading System v2.
All tunable parameters in one place.
"""

SIM_DURATION_MINUTES = 120
POLL_INTERVAL_SECONDS = 5
INITIAL_CAPITAL = 100.0
MIN_CAPITAL_TO_TRADE = 1.0

GAMMA_API = "https://gamma-api.polymarket.com"
CLOB_API = "https://clob.polymarket.com"
DATA_API = "https://data-api.polymarket.com"

MIN_VOLUME_USD = 1000
MIN_PRICE = 0.01
MAX_PRICE = 0.99

MAKER_REBATE_RATE = 0.001
TAKER_FEE_RATE = 0.02

DEFAULT_SPREAD_BPS = 50
LIQUID_SPREAD_BPS = 25
ILLIQUID_SPREAD_BPS = 100
ORDER_BOOK_DEPTH = 10

DEFAULT_STRATEGY = {
    "min_liquidity": 500,
    "max_legs": 2,
    "min_spread_bps": 20,
    "bid_offset_bps": 5,
    "ask_offset_bps": 5,
    "post_both_sides": True,
    "max_position_pct": 0.10,
    "min_position_size": 0.50,
    "max_position_size": 10.0,
    "max_daily_loss_pct": 0.05,
    "max_positions": 5,
    "cancel_after_seconds": 300,
    "fill_probability": 0.30,
}

POPULATION_SIZE = 20
GENERATIONS = 30
MUTATION_RATE = 0.2
ELITE_COUNT = 3
TOURNAMENT_SIZE = 4

MAX_TRADE_SIZE_LIVE = 5.0
DRY_RUN = True
MAX_DAILY_SPEND = 20.0

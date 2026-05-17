"""
MuonAdamW optimizer — ported verbatim from autoresearch/train.py.

autoreseach single-file harness:
  - Muon: MUtating Optimizer with Orthogonal Normalization
  - AdamW for 1-D params (bias, layernorm, embeddings)
  - Muon for 2-D params (weight matrices)

GYM usage:
  from core.gym.muon import MuonAdamW
  opt = MuonAdamW(param_groups)
  opt.step()
  opt.zero_grad()

autoreseach reference: train.py lines 294–411.
polar_express_coeffs copied verbatim from train.py lines 297–303.
"""
from __future__ import annotations
import math
from typing import List, Tuple, Dict, Any, Optional

# ── Polar Express orthogonalization coefficients ───────────────────────────────
# Source: train.py lines 297–303
POLAR_EXPRESS_COEFFS: List[Tuple[float, float, float]] = [
    (8.156554524902461, -22.48329292557795, 15.878769915207462),
    (4.042929935166739, -2.808917465908714, 0.5000178451051316),
    (3.8916678022926607, -2.772484153217685, 0.5060648178503393),
    (3.285753657755655, -2.3681294933425376, 0.46449024233003106),
    (2.3465413258596377, -1.7097828382687081, 0.42323551169305323),
]

# ── Hard optimum LRs (from train.py setup_optimizer defaults) ─────────────────
DEFAULT_EMBEDDING_LR = 0.2
DEFAULT_UNEMBEDDING_LR = 0.004
DEFAULT_MATRIX_LR = 0.02
DEFAULT_SCALAR_LR = 0.5
DEFAULT_ADAM_BETAS = (0.8, 0.95)
DEFAULT_WEIGHT_DECAY = 0.0


class MuonAdamW:
    """
    Combined optimizer: Muon for 2D matrix params, AdamW for others.
    Ported from train.py lines 413–458.

    Usage (mirror of model.setup_optimizer in train.py):

        param_groups = [
            dict(kind='muon',   params=matrix_params,  lr=0.04, momentum=0.95, ...),
            dict(kind='adamw',  params=embedding_params, lr=0.2,  betas=(0.8,0.95), ...),
            dict(kind='adamw',  params=resid_params,     lr=0.005, betas=(0.96,0.95), ...),
        ]
        opt = MuonAdamW(param_groups)
        opt.step()
    """

    def __init__(self, param_groups: List[Dict[str, Any]]):
        # CPU tensors avoid recompilation when values change (train.py pattern)
        self._adamw_step_t = _scalar(0.0)
        self._adamw_lr_t   = _scalar(0.0)
        self._adamw_beta1_t = _scalar(0.0)
        self._adamw_beta2_t = _scalar(0.0)
        self._adamw_eps_t   = _scalar(0.0)
        self._adamw_wd_t    = _scalar(0.0)
        self._muon_mom_t    = _scalar(0.0)
        self._muon_lr_t     = _scalar(0.0)
        self._muon_wd_t     = _scalar(0.0)
        self._muon_beta2_t  = _scalar(0.0)
        self.param_groups = param_groups
        self.state: Dict[Any, Dict] = {}

    def step(self):
        for group in self.param_groups:
            kind = group.get("kind", "adamw")
            if kind == "adamw":
                self._step_adamw(group)
            elif kind == "muon":
                self._step_muon(group)

    def zero_grad(self, set_to_none: bool = True):
        for group in self.param_groups:
            for p in group.get("params", []):
                if p in self.state and set_to_none:
                    self.state[p]["exp_avg"] = None
                    self.state[p]["exp_avg_sq"] = None

    def _step_adamw(self, group: Dict[str, Any]):
        lr = group["lr"]
        betas = group["betas"]
        eps = group["eps"]
        wd = group.get("weight_decay", 0.0)

        self._adamw_lr_t.fill_(lr)
        self._adamw_beta1_t.fill_(betas[0])
        self._adamw_beta2_t.fill_(betas[1])
        self._adamw_eps_t.fill_(eps)
        self._adamw_wd_t.fill_(wd)

        for p in group.get("params", []):
            grad = getattr(p, "grad", None)
            if grad is None:
                continue
            state = self.state.get(p)
            if state is None:
                state = {"step": 0, "exp_avg": None, "exp_avg_sq": None}
                self.state[p] = state
            step = state["step"] + 1
            state["step"] = step

            g = grad.float()

            # exp_avg: EMA of gradients
            if state["exp_avg"] is None:
                state["exp_avg"] = g.clone()
            else:
                state["exp_avg"] = state["exp_avg"].lerp_(g, 1 - betas[0])

            # exp_avg_sq: EMA of gradient squared
            if state["exp_avg_sq"] is None:
                state["exp_avg_sq"] = g.square()
            else:
                state["exp_avg_sq"] = state["exp_avg_sq"].lerp_(g.square(), 1 - betas[1])

            exp_avg = state["exp_avg"]
            exp_avg_sq = state["exp_avg_sq"]

            # Bias correction
            bias1 = 1 - betas[0] ** step
            bias2 = 1 - betas[1] ** step

            denom = (exp_avg_sq / bias2).sqrt() + eps
            step_size = lr / bias1

            # Weight decay
            if wd > 0:
                p.data.mul_(1 - lr * wd)

            # Parameter update
            p.data.add_(exp_avg / denom, alpha=-step_size)

    def _step_muon(self, group: Dict[str, Any]):
        torch = _require_torch()
        params = group.get("params", [])
        if not params:
            return

        p0 = params[0]
        state0 = self.state.get(p0, {})
        self.state[p0] = state0

        shape = p0.shape
        device = p0.device
        dtype = p0.dtype
        red_dim = -1 if shape[-2] >= shape[-1] else -2

        # Stack grads and params for efficient batch update
        grads = [getattr(p, "grad", None) for p in params]
        if all(g is None for g in grads):
            return

        valid_grads = [g for g in grads if g is not None]
        valid_params = [p for p in params if getattr(p, "grad", None) is not None]

        stacked_grads = torch.stack(valid_grads)
        stacked_params = torch.stack(valid_params)

        # Momentum buffer
        num = len(valid_params)
        if "momentum_buffer" not in state0:
            state0["momentum_buffer"] = torch.zeros(num, *shape, dtype=dtype, device=device)
        if "second_momentum_buffer" not in state0:
            state_shape = (num, shape[-2], 1) if shape[-2] >= shape[-1] else (num, 1, shape[-1])
            state0["second_momentum_buffer"] = torch.zeros(state_shape, dtype=dtype, device=device)

        mom_buf = state0["momentum_buffer"]
        sec_mom = state0["second_momentum_buffer"]

        lr = group["lr"]
        mom = group.get("momentum", 0.95)
        beta2 = group.get("beta2", 0.99)
        wd = group.get("weight_decay", 0.0)
        ns_steps = group.get("ns_steps", 5)

        # Scaled LR (sqrt aspect ratio correction, train.py pattern)
        scaled_lr = lr * max(1.0, shape[-2] / shape[-1]) ** 0.5

        # ── Nesterov momentum ──────────────────────────────────────
        mom_buf.lerp_(stacked_grads, 1 - mom)
        g = stacked_grads.lerp_(mom_buf, mom)

        # ── Polar express orthogonalization ──────────────────────────
        X = g.bfloat16()
        X_norm = X.norm(dim=(-2, -1), keepdim=True)
        X = X / (X_norm * 1.02 + 1e-6)

        if shape[-2] >= shape[-1]:
            # Tall matrix: (rows, cols), compute X^T @ X
            for a, b, c in POLAR_EXPRESS_COEFFS[:ns_steps]:
                A = X.mT @ X
                B = b * A + c * (A @ A)
                X = a * X + X @ B
        else:
            # Wide matrix: (rows, cols), compute X @ X^T
            for a, b, c in POLAR_EXPRESS_COEFFS[:ns_steps]:
                A = X @ X.mT
                B = b * A + c * (A @ A)
                X = a * X + B @ X

        g = X.to(dtype=g.dtype)

        # ── NorMuon variance reduction ───────────────────────────────
        v_mean = g.float().square().mean(dim=red_dim, keepdim=True)
        red_dim_size = g.size(red_dim)

        v_norm_sq = v_mean.sum(dim=(-2, -1), keepdim=True) * red_dim_size
        v_norm = v_norm_sq.sqrt()

        sec_mom.lerp_(v_mean.to(dtype=sec_mom.dtype), 1 - beta2)
        step_size = sec_mom.clamp_min(1e-10).rsqrt()

        scaled_sq_sum = (v_mean * red_dim_size) * step_size.float().square()
        v_norm_new = scaled_sq_sum.sum(dim=(-2, -1), keepdim=True).sqrt()
        final_scale = step_size * (v_norm / v_norm_new.clamp_min(1e-10))
        g = g * final_scale.to(g.dtype)

        # ── Cautious weight decay + parameter update ─────────────────
        mask = (g * stacked_params) >= 0
        stacked_params = stacked_params - scaled_lr * (g + scaled_lr * wd * stacked_params * mask.to(dtype=stacked_params.dtype))

        # Copy back
        for i, p in enumerate(valid_params):
            p.data.copy_(stacked_params[i])

    def get_state_snapshot(self) -> dict:
        """Return a serializable dict of optimizer state for checkpointing."""
        return {
            "param_groups": [
                {k: v for k, v in g.items() if k != "params"}
                for g in self.param_groups
            ],
            "step_counts": {
                id(p): s["step"]
                for p, s in self.state.items()
            },
        }


def _require_torch():
    """Import torch only when the optimizer is actually instantiated/stepped.

    Makakoo ships GYM metadata without a torch dependency. Importing this
    module must stay quiet on user machines that either lack torch or have a
    broken optional torch/numpy stack.
    """
    try:
        import torch as _torch
    except Exception as exc:  # torch can raise non-ImportError during init
        raise RuntimeError("MuonAdamW requires torch at runtime; importing core.gym.muon does not") from exc
    return _torch


def _scalar(value: float):
    """CPU 0-D tensor — avoids recompilation on value change (train.py pattern)."""
    torch = _require_torch()
    return torch.tensor(value, dtype=torch.float32, device="cpu")


def setup_optimizer(
    matrix_params: List[Any],
    embedding_params: List[Any],
    unembedding_params: List[Any] | None = None,
    scalar_params: List[Any] | None = None,
    value_embeds_params: List[Any] | None = None,
    embedding_lr: float = DEFAULT_EMBEDDING_LR,
    unembedding_lr: float = DEFAULT_UNEMBEDDING_LR,
    matrix_lr: float = DEFAULT_MATRIX_LR,
    scalar_lr: float = DEFAULT_SCALAR_LR,
    weight_decay: float = DEFAULT_WEIGHT_DECAY,
    adam_betas: Tuple[float, float] = DEFAULT_ADAM_BETAS,
    model_dim: int = 768,
) -> MuonAdamW:
    """
    Build param groups matching train.py's setup_optimizer().

    LR scaling (tuned at 768 dim): 1/sqrt(model_dim/768)
    This ensures LR is consistent across model sizes.
    """
    dmodel_lr_scale = (model_dim / 768) ** -0.5

    groups = []

    # Muon: 2D matrix params (all transformer weight matrices)
    for p in matrix_params:
        groups.append(dict(
            kind="muon",
            params=[p],
            lr=matrix_lr,
            momentum=0.95,
            ns_steps=5,
            beta2=0.95,
            weight_decay=weight_decay,
        ))

    # AdamW: token embeddings
    if embedding_params:
        groups.append(dict(
            kind="adamw",
            params=embedding_params,
            lr=embedding_lr * dmodel_lr_scale,
            betas=adam_betas,
            eps=1e-10,
            weight_decay=0.0,
        ))

    # AdamW: value embeddings
    if value_embeds_params:
        groups.append(dict(
            kind="adamw",
            params=value_embeds_params,
            lr=embedding_lr * dmodel_lr_scale,
            betas=adam_betas,
            eps=1e-10,
            weight_decay=0.0,
        ))

    # AdamW: lm_head (unembedding)
    if unembedding_params:
        groups.append(dict(
            kind="adamw",
            params=unembedding_params,
            lr=unembedding_lr * dmodel_lr_scale,
            betas=adam_betas,
            eps=1e-10,
            weight_decay=0.0,
        ))

    # AdamW: scalar params (resid_lambdas, x0_lambdas)
    if scalar_params:
        groups.append(dict(
            kind="adamw",
            params=scalar_params,
            lr=scalar_lr,
            betas=(0.96, 0.95),
            eps=1e-10,
            weight_decay=0.0,
        ))

    opt = MuonAdamW(groups)
    # Store initial LRs for schedule adjustments
    for group in opt.param_groups:
        group["initial_lr"] = group["lr"]
    return opt

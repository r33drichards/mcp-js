#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "matplotlib",
#     "numpy",
# ]
# ///
"""Generate benchmark comparison graphs from results data."""

import json
import os

import matplotlib.pyplot as plt
import numpy as np

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))


def load_data():
    with open(os.path.join(SCRIPT_DIR, "benchmark_results.json")) as f:
        return json.load(f)


def plot_bar_chart(data):
    """Bar chart of average total tokens by approach."""
    approaches = ["A", "B", "C"]
    labels = [data[a]["label"] for a in approaches]
    avgs = [data[a]["avg"]["total"] for a in approaches]
    colors = ["#e74c3c", "#3498db", "#2ecc71"]

    fig, ax = plt.subplots(figsize=(8, 5))
    bars = ax.bar(labels, avgs, color=colors, width=0.6, edgecolor="white", linewidth=1.5)

    for bar, val in zip(bars, avgs):
        ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height() + 2000,
                f"{val:,.0f}", ha="center", va="bottom", fontsize=12, fontweight="bold")

    ax.set_ylabel("Average Total Tokens", fontsize=12)
    ax.set_title("Token Usage by Approach (10-run average)", fontsize=14, fontweight="bold")
    ax.set_ylim(0, max(avgs) * 1.15)
    ax.yaxis.set_major_formatter(plt.FuncFormatter(lambda x, _: f"{x:,.0f}"))
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    plt.tight_layout()
    plt.savefig(os.path.join(SCRIPT_DIR, "graph_avg_tokens.png"), dpi=150)
    plt.close()


def plot_box_chart(data):
    """Box plot showing variance across runs."""
    approaches = ["A", "B", "C"]
    labels = [data[a]["label"] for a in approaches]
    totals = [[r["total"] for r in data[a]["runs"]] for a in approaches]
    colors = ["#e74c3c", "#3498db", "#2ecc71"]

    fig, ax = plt.subplots(figsize=(8, 5))
    bp = ax.boxplot(totals, tick_labels=labels, patch_artist=True, widths=0.5,
                    medianprops=dict(color="black", linewidth=2))
    for patch, color in zip(bp["boxes"], colors):
        patch.set_facecolor(color)
        patch.set_alpha(0.7)

    # Overlay individual points
    for i, (tots, color) in enumerate(zip(totals, colors)):
        jitter = np.random.normal(0, 0.04, len(tots))
        ax.scatter([i + 1 + j for j in jitter], tots, color=color,
                   edgecolors="black", s=40, zorder=5, alpha=0.8)

    ax.set_ylabel("Total Tokens per Run", fontsize=12)
    ax.set_title("Token Usage Distribution (10 runs each)", fontsize=14, fontweight="bold")
    ax.yaxis.set_major_formatter(plt.FuncFormatter(lambda x, _: f"{x:,.0f}"))
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    plt.tight_layout()
    plt.savefig(os.path.join(SCRIPT_DIR, "graph_distribution.png"), dpi=150)
    plt.close()


def plot_per_run(data):
    """Line chart showing tokens per run for each approach."""
    colors = {"A": "#e74c3c", "B": "#3498db", "C": "#2ecc71"}

    fig, ax = plt.subplots(figsize=(10, 5))
    for key in ["A", "B", "C"]:
        runs = data[key]["runs"]
        x = list(range(1, len(runs) + 1))
        y = [r["total"] for r in runs]
        ax.plot(x, y, marker="o", label=data[key]["label"], color=colors[key],
                linewidth=2, markersize=6)

    ax.set_xlabel("Run Number", fontsize=12)
    ax.set_ylabel("Total Tokens", fontsize=12)
    ax.set_title("Token Usage per Run", fontsize=14, fontweight="bold")
    ax.legend(fontsize=10)
    ax.set_xticks(range(1, 11))
    ax.yaxis.set_major_formatter(plt.FuncFormatter(lambda x, _: f"{x:,.0f}"))
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    plt.tight_layout()
    plt.savefig(os.path.join(SCRIPT_DIR, "graph_per_run.png"), dpi=150)
    plt.close()


def main():
    data = load_data()
    plot_bar_chart(data)
    print("Created graph_avg_tokens.png")
    plot_box_chart(data)
    print("Created graph_distribution.png")
    plot_per_run(data)
    print("Created graph_per_run.png")


if __name__ == "__main__":
    main()

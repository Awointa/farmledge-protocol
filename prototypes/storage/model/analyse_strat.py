def strat(threshold, extend_to, life):
    """Days of TTL bought after the write (which prepays 175 days), the
    number of renewals, and the TTL left over, for an entry living `life`
    days under a daily job that renews it to `extend_to` days once fewer
    than `threshold` days remain."""
    live, renewals = 175, 0
    for day in range(1, life + 1):
        if live - day < threshold:
            live, renewals = day + extend_to, renewals + 1
    return live - 175, renewals, live - life

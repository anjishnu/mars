The captain has just returned to their terminal after being away. Below is everything that was on their panes while they were gone. Write their situation report as EXACTLY FOUR short blocks, each separated by a blank line:

1. A brief greeting to the captain — one or two sentences. Vary it; do not open the same way every time. Set the tone.

2. A plain-English summary of what actually happened across the panes: what ran, what finished, what is still going. INTERPRET the output into human language — do not recite raw numbers. Say "the run finished at its best accuracy so far" or "it's about two-thirds through the sweep", not "val_ndcg 0.71, config 7/10". Quote a specific number ONLY when it is the single key fact and you immediately say what it means; never list raw counts, step indices, config numbers, or metrics the captain cannot interpret at a glance. Keep genuinely meaningful terms (a named test, an error type, a file path) where a vaguer word would lose meaning, and name the specific pane. If a long-running job SUCCEEDED while they were away (a training run, a build, a migration), treat it as the win it is. Where it matters, note progress since the last briefing: if something you flagged before is now resolved or still open, say so briefly ("the OOM you were chasing is still red").

3. The action items: anything waiting on the captain's input, blocked on a choice, or failed in a way that needs a decision, stated plainly so they know exactly what to do next. If everything succeeded and nothing needs them, say so plainly.

4. A single closing sign-off sentence — the last word. If the board is clean, allow a dry beat of wit and let it lead with the best news. If something is on fire, drop the wit: be steady and point at the first move. Keep it to one sentence.

No markdown, no headings, no bullet lists, no preamble like "here is". Plain prose in four blocks, a blank line between each. Keep it tight — each block is one to three sentences.

Time away: {away}
Mission: {mission}
Last briefing: {prev}

{evidence}

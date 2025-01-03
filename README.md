# glommactor
An simple actor framework for the glommio runtime. This crate is for learning purposes only. It is heavily inspired by [act-zero](https://www.github.com/Diggsey/act-zero) and Alice Ryhl's
[tokio actors](https://ryhl.io/blog/actors-with-tokio/). Honorable mention to Erlang's Actor model as well.

# Comparison to Other Actor Frameworks

The key difference between glommactor and other actor frameworks is that it is
intended to run within [glommio, a thread-per-core runtime](https://www.github.com/DataDogHQ/glommio). Almost all of the (reasonably usable) actor crates with async support I found are not runtime agnostic; most assume tokio with a handful supporting async-std. The standout exception here is the [act-zero crate](https://www.github.com/Diggsey/act-zero), which in general is an excellent runtime-agnostic actor framework. However I found that act-zero does not support all my desired use cases. This of course is solveable, but the crate also seems to not really be maintained anymore. So I decided to build something simple to support my use cases while learning more about actors.

# Usecases

The main usecase I wanted to build for is allowing messaging/actor interaction across cores. So the goal is to have an actor that is pinned to a specific core, where that actor can be interacted with from a handle within tasks executing on separate cores, as well as on the actor-pinned core. This is mainly intended for command and control but could potentially also be used for data processing in limited scenarios (since this arch is intended for zero-sharing between cores). 

# Benchmarks

To get some simple comparison benchmarks I implemented glommio support in a [fork of the act-zero crate](https://github.com/nand-nor/act-zero/tree/add/glommio) and have used that for the benchmarks listed below. 

The OS is WSL (5.15.167.4-microsoft-standard-WSL2 #1 SMP x86_64) using cores 0, 1, 2, and 3 of a 12th Gen Intel(R) Core(TM) i9-12900K CPU: 
```
     Running benches/actors.rs (target/release/deps/actors-b03984d68620e47f)

running 2 tests
test test_actzero ... bench:   2,342,768 ns/iter (+/- 831,846)
test test_glom    ... bench:   2,474,159 ns/iter (+/- 845,877)

test result: ok. 0 passed; 0 failed; 0 ignored; 2 measured
```
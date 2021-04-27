extern crate x8;
use colored::*;
use reqwest::Client;
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    time::Duration,
};
use x8::{
    args::get_config,
    logic::cycles,
    requests::{empty_reqs, random_request, request},
    structs::Config,
    utils::{compare, generate_data, heuristic, make_hashmap, random_line, read_lines},
};

fn main() {
    //colored::control::set_override(true);
    //saves false-positive diffs
    let mut green_lines: HashMap<String, usize> = HashMap::new();

    let (config, mut max): (Config, usize) = get_config();
    if config.verbose > 0 && !config.test {
        writeln!(
            io::stdout(),
            " _________  __ ___     _____\n|{} {}",
            &config.method.blue(),
            &config.url.green(),
        ).ok();
    }

    if !config.proxy.is_empty() && config.verbose > 0 && !config.test {
        writeln!(
            io::stdout(),
            "|{} {}",
            "Proxy".magenta(),
            &config.proxy.green(),
        ).ok();
    }

    if !config.save_responses.is_empty() {
        match fs::create_dir(&config.save_responses) {
            Ok(_) => (),
            Err(err) => {
                writeln!(
                    io::stderr(),
                    "Unable to create a directory '{}' due to {}",
                    &config.save_responses,
                    err
                ).unwrap_or(());
                std::process::exit(1);
            }
        };
    }

    let mut params: Vec<String> = Vec::new();

    //read parameters from a file
    if let Ok(lines) = read_lines(&config.wordlist) {
        for line in lines.flatten() {
            let val = line;
            params.push(val);
        }
    }

    //build clients
    let mut client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(60))
        .max_idle_per_host(10)
        .http1_title_case_headers()
        .cookie_store(true);

    if !config.proxy.is_empty() {
        client = client.proxy(reqwest::Proxy::all(&config.proxy).unwrap());
    }

    if !config.follow_redirects {
        client = client.redirect(reqwest::RedirectPolicy::none());
    }

    let client = client.build().unwrap();

    let mut replay_client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(60))
        .max_idle_per_host(10)
        .http1_title_case_headers()
        .cookie_store(true);

    if !config.replay_proxy.is_empty() {
        replay_client = replay_client.proxy(reqwest::Proxy::all(&config.replay_proxy).unwrap());
    }

    if !config.follow_redirects {
        replay_client = replay_client.redirect(reqwest::RedirectPolicy::none());
    }

    let replay_client = replay_client.build().unwrap();

    //generate random query for the first request
    let query = make_hashmap(
        &(0..max).map(|_| random_line(config.value_size)).collect::<Vec<String>>(),
        &config.value_template,
        &config.key_template,
        config.value_size,
    );

    //get cookies
    request(&config, &client, &HashMap::new(), 0);

    // if opened in the test mode - generate request/response and quit
    if config.test {
        generate_data(&config, &client, &query);
        std::process::exit(0)
    }

    // make first request and collect some information like code, reflections, possible parameters
    let mut initial_response = request(&config, &client, &query, 0);

    if initial_response.code == 0 {
        writeln!(io::stderr(), "Unable to reach - {} ", &config.url).ok();
        std::process::exit(1)
    }

    params.append(&mut heuristic(&initial_response.text));

    if params.len() < max {
        max = params.len();
        if max == 0 {
            writeln!(io::stderr(), "Parameter list is empty").ok();
            std::process::exit(1)
        }
    }

    //get reflection count
    initial_response.reflected_params = Vec::new();

    let reflections_count = initial_response
        .text
        .matches(&query.values().next().unwrap().replace("%random%_", "").as_str())
        .count() as usize;

    writeln!(
        io::stdout(),
        "|{} {}\n|{} {}\n|{} {}\n|{} {}\n",
        &"Code".magenta(),
        &initial_response.code.to_string().green(),
        &"Response Len".magenta(),
        &initial_response.text.len().to_string().green(),
        &"Reflections".magenta(),
        &reflections_count.to_string().green(),
        &"Words".magenta(),
        &params.len().to_string().green(),
    ).ok();

    //make a few requests and collect all persistent diffs, check for stability
    let (mut diffs, stable) = empty_reqs(
        &config,
        &initial_response,
        reflections_count,
        config.learn_requests_count,
        &client,
        max,
    );

    //check whether it is possible to use 192 or 256 params in a single request instead of 128 default
    if max == 128 {
        let response = random_request(&config, &client, reflections_count, max + 64);

        let (is_code_the_same, new_diffs) = compare(&config, &initial_response, &response);
        let mut is_the_body_the_same = true;

        for diff in new_diffs.iter() {
            if !diffs.iter().any(|i| i == diff) {
                is_the_body_the_same = false;
            }
        }

        if is_code_the_same && (!stable.body || is_the_body_the_same) {
            let response = random_request(&config, &client, reflections_count, max + 128);
            let (is_code_the_same, new_diffs) = compare(&config, &initial_response, &response);

            for diff in new_diffs {
                if !diffs.iter().any(|i| i == &diff) {
                    is_the_body_the_same = false;
                }
            }

            if is_code_the_same && (!stable.body || is_the_body_the_same) {
                max += 128
            } else {
                max += 64
            }
            if config.verbose > 0 {
                writeln!(
                    io::stdout(),
                    "[#] the max number of parameters in every request was increased to {}",
                    max
                ).ok();
            }
        }
    }

    let mut custom_parameters: HashMap<String, Vec<String>> = config.custom_parameters.clone();
    let mut remaining_params: Vec<Vec<String>> = Vec::new();
    let mut found_params: Vec<String> = Vec::new();
    let mut first: bool = true;
    let initial_size: usize = params.len() / max;
    let mut count: usize = 0;

    loop {
        cycles(
            first,
            &config,
            &initial_response,
            &mut diffs,
            &params,
            &stable,
            reflections_count,
            &client,
            max,
            &mut green_lines,
            &mut remaining_params,
            &mut found_params,
        );
        first = false;
        count += 1;

        if count > 100
            || (count > 50 && remaining_params.len() < 10)
            || (count > 10 && remaining_params.len() > (initial_size / 2 + 5))
            || (count > 1 && remaining_params.len() > (initial_size * 2 + 10))
        {
            writeln!(io::stderr(), "{} Infinity loop detected", config.url).ok();
            std::process::exit(1);
        }

        params = Vec::with_capacity(remaining_params.len() * max);
        max /= 2;

        if max == 0 {
            max = 1;
        }

        //if there is a parameter in remaining_params that also exists in found_params - ignore it.
        let mut found: bool = false;
        for vector_params in &remaining_params {
            for param in vector_params {
                for found_param in &found_params {
                    //some strange logic in order to treat admin=1 and admin=something as the same parameters
                    let param_key = if param.matches('=').count() == 1 {
                        param.split('=').next().unwrap()
                    } else {
                        param
                    };

                    if found_param == param_key
                        || found_param.matches('=').count() == 1
                        && found_param.split('=').next().unwrap() == param_key {
                        found = true;
                        break;
                    }
                }
                if !found {
                    params.push(param.to_string());
                }
                found = false;
            }
        }

        if params.is_empty() && !config.disable_custom_parameters {
            max = config.max;
            for (k, v) in custom_parameters.iter_mut() {
                if !v.is_empty() {
                    params.push([k.as_str(), "=", &v.pop().unwrap().as_str()].concat());
                }
            }
            if max > params.len() {
                max = params.len()
            }
        }

        if params.is_empty() {
            break;
        }

        remaining_params = Vec::new()
    }

    found_params.sort();
    found_params.dedup();

    if !config.replay_proxy.is_empty() {
        let temp_config = Config{
            disable_cachebuster: true,
            ..config.clone()
        };

        request(&temp_config, &replay_client, &HashMap::new(), 0);

        if config.replay_once {
            request(
                &temp_config,
                &replay_client,
                &make_hashmap(
                    &found_params,
                    &config.value_template,
                    &config.key_template,
                    config.value_size
                ),
                0
            );
        } else {
            for param in &found_params {
                request(
                    &temp_config,
                    &replay_client,
                    &make_hashmap(
                        &[param.to_owned()],
                        &config.value_template,
                        &config.key_template,
                        config.value_size
                    ),
                    0
                );
            }
        }
    }

    //TODO different output types
    let mut output = format!("{} {} % ", &config.method, &config.url);

    /*if config.verify {
        let mut filtered_params = Vec::new();
        for param in found_params {

            let response = request(
                &config, &client,
                &random_hashmap(
                    &[param.clone()], &config.value_template, &config.key_template
                ),
                reflections_count
            );

            let (is_code_the_same, new_diffs) = compare(&config, &initial_response, &response);
            let mut is_the_body_the_same = true;

            for diff in new_diffs.iter() {
                if !diffs.iter().any(|i| &i==&diff) {
                    is_the_body_the_same = false;
                }
            }

            if !response.reflected_params.is_empty() || !is_the_body_the_same || !is_code_the_same {
                filtered_params.push(param);
            }
        }
        found_params = filtered_params;
    }*/

    for param in &found_params {
        output.push_str(&param);
        output.push_str(", ")
    }

    let mut output = output[..output.len() - 2].to_string();
    output.push('\n');

    if !config.output_file.is_empty() && !found_params.is_empty() {
        match std::fs::write(&config.output_file, &output) {
            Ok(_) => (),
            Err(err) => {
                writeln!(io::stderr(), "[!] {}", err).ok();
            }
        };
    }
    writeln!(io::stdout(), "{}", &output).ok();
}